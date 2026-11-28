// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Move to contacts folder (#1144): every pair of places, a failure between the two steps, undo
//! at each stage, folders that may only be read.
//!
//! The invariant every test keeps: a contact is never in neither place. The copy a server's folder
//! has is deleted there only after the server has made the new one (an answer with its id, or a
//! download that shows it); a refusal puts it back where it was.

use mailcraft_model::contact_sync::{
    ContactChangeKind, ContactOp, ContactOutcome, ContactResult, ContactsUpload, ContactsUploaded, FolderOwner, RemoteContactFolder,
};
use mailcraft_model::remote::{ContactsDownload, RemoteContact};
use mailcraft_model::{Id, Labeled};
use serde_json::{Value, json};

use crate::Session;

const NOW: i64 = 1_790_000_000;
const ADA: &str = "ada@example.org";
const EVE: &str = "eve@example.org";

/// The sample mailbox with two Microsoft accounts: Ada's server has Bo and Cy in its main folder
/// and Di in a "Team" folder (one more of Ada's that may be written), Eve's has Fay.
struct Rig {
    s: Session,
    ada: Id,
    eve: Id,
}

impl Rig {
    fn new() -> Self {
        let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
        let ada = add(&mut s, ADA);
        let eve = add(&mut s, EVE);
        run(&mut s, "sync.applyContacts", json!({"download": download(ada, ADA, vec![person("c-bo", "Bo"), person("c-cy", "Cy")])}));
        run(&mut s, "sync.applyContacts", json!({"download": team(ada, true, vec![person("c-di", "Di")])}));
        run(&mut s, "sync.applyContacts", json!({"download": download(eve, EVE, vec![person("e-fay", "Fay")])}));
        Self { s, ada, eve }
    }

    fn called(&self, first: &str) -> Id {
        self.s.mailbox.contacts.values().find(|c| c.first == first).map(|c| c.id).unwrap_or_else(|| panic!("no {first}"))
    }

    fn folder_of(&self, first: &str) -> Id {
        self.s.mailbox.contact(self.called(first)).map(|c| c.folder).unwrap_or_default()
    }

    fn main(&self, account: Id) -> Id {
        self.s.mailbox.main_contact_folder(account).unwrap()
    }

    fn team(&self) -> Id {
        self.s.mailbox.contact_folder_named(self.ada, &FolderOwner::Mailbox("team@example.org".into()), "f-team").unwrap()
    }

    fn mv(&mut self, ids: &[Id], folder: Id) -> Value {
        run(&mut self.s, "people.moveToFolder", json!({"ids": ids, "folder": folder}))
    }

    fn upload(&mut self, account: Id) -> ContactsUpload {
        serde_json::from_value(run(&mut self.s, "sync.contactChanges", json!({"account": account}))).unwrap()
    }

    fn answer(&mut self, account: Id, op: &ContactOp, result: ContactResult, remote: &str) -> Value {
        let email = if account == self.ada { ADA } else { EVE };
        let outcome = ContactOutcome { op: op.clone(), result, remote: remote.into(), change_key: "ck-new".into(), ..Default::default() };
        let uploaded = ContactsUploaded { account, email: email.into(), outcomes: vec![outcome], ..Default::default() };
        run(&mut self.s, "sync.applyContactChanges", json!({"result": uploaded}))
    }

    /// The ops due for `account`, as (kind, remote id).
    fn due(&mut self, account: Id) -> Vec<(ContactChangeKind, String)> {
        self.upload(account).ops.into_iter().map(|op| (op.kind, op.remote)).collect()
    }

    /// How many contacts called `first` there are, and in which folders.
    fn places(&self, first: &str) -> Vec<Id> {
        let mut v: Vec<Id> = self.s.mailbox.contacts.values().filter(|c| c.first == first).map(|c| c.folder).collect();
        v.sort_unstable();
        v
    }
}

fn add(s: &mut Session, email: &str) -> Id {
    run(s, "account.add", json!({"email": email, "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"}))["id"].as_u64().unwrap()
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

fn download(account: Id, email: &str, contacts: Vec<RemoteContact>) -> ContactsDownload {
    ContactsDownload { account, email: email.into(), complete: true, contacts, ..Default::default() }
}

fn team(account: Id, writable: bool, contacts: Vec<RemoteContact>) -> ContactsDownload {
    let folder = RemoteContactFolder {
        id: "f-team".into(),
        name: "Team".into(),
        owner: FolderOwner::Mailbox("team@example.org".into()),
        writable,
        ..Default::default()
    };
    ContactsDownload { folder: Some(folder), ..download(account, ADA, contacts) }
}

#[test]
fn from_this_computer_into_an_accounts_folder_is_made_there_and_nothing_is_deleted() {
    let mut r = Rig::new();
    let gus = run(&mut r.s, "people.new", json!({"first": "Gus", "folder": 0}))["id"].as_u64().unwrap();
    let main = r.main(r.ada);
    let said = r.mv(&[gus], main);
    assert_eq!(said["moved"][0], json!({"from": gus, "id": gus}));
    assert_eq!((r.places("Gus"), r.s.mailbox.contact(gus).unwrap().origin.account), (vec![main], r.ada));
    assert_eq!(r.due(r.ada), [(ContactChangeKind::Create, String::new())]);
    let op = r.upload(r.ada).ops.remove(0);
    r.answer(r.ada, &op, ContactResult::Done, "c-gus");
    assert!(r.s.mailbox.contact_changes.is_empty());
    assert_eq!(r.s.mailbox.contact(gus).unwrap().origin.id, "c-gus");
}

#[test]
fn between_two_accounts_the_source_is_deleted_only_once_the_target_has_made_it() {
    let mut r = Rig::new();
    let bo = r.called("Bo");
    r.s.sel.contact = Some(bo);
    let eve_main = r.main(r.eve);
    let said = r.mv(&[bo], eve_main);
    let moved = said["moved"][0]["id"].as_u64().unwrap();
    assert_ne!(moved, bo);
    // Moved here at once, and the selection with it.
    assert_eq!((r.places("Bo"), r.s.sel.contact), (vec![eve_main], Some(moved)));
    assert_eq!(r.s.mailbox.contact(moved).unwrap().origin.account, r.eve);
    // Ada's server isn't told to delete Bo yet: Eve's hasn't made him.
    assert!(r.due(r.ada).is_empty());
    // A download of Ada's folder meanwhile doesn't bring Bo back.
    run(&mut r.s, "sync.applyContacts", json!({"download": download(r.ada, ADA, vec![person("c-bo", "Bo"), person("c-cy", "Cy")])}));
    assert_eq!(r.places("Bo"), vec![eve_main]);
    let op = r.upload(r.eve).ops.remove(0);
    assert_eq!((op.kind, op.values.first.as_str()), (ContactChangeKind::Create, "Bo"));
    r.answer(r.eve, &op, ContactResult::Done, "e-bo");
    // Made there: now Ada's copy goes.
    assert_eq!(r.due(r.ada), [(ContactChangeKind::Delete, "c-bo".to_string())]);
    let op = r.upload(r.ada).ops.remove(0);
    r.answer(r.ada, &op, ContactResult::Done, "");
    assert!(r.s.mailbox.contact_changes.is_empty());
    assert_eq!(r.s.mailbox.contact(moved).unwrap().origin.id, "e-bo");
}

#[test]
fn between_two_folders_of_one_account_the_delete_waits_for_the_create() {
    let mut r = Rig::new();
    let (cy, team) = (r.called("Cy"), r.team());
    run(&mut r.s, "people.edit", json!({"id": cy, "last": "Lee"}));
    let moved = r.mv(&[cy], team)["moved"][0]["id"].as_u64().unwrap();
    // The edit not sent yet travels with it.
    assert_eq!(r.s.mailbox.contact(moved).unwrap().last, "Lee");
    let ops = r.upload(r.ada).ops;
    let [op] = ops.as_slice() else { panic!("only the create is due: {ops:?}") };
    assert_eq!((op.kind, op.folder.as_str()), (ContactChangeKind::Create, "f-team"));
    r.answer(r.ada, op, ContactResult::Done, "c-cy-team");
    assert_eq!(r.due(r.ada), [(ContactChangeKind::Delete, "c-cy".to_string())]);
}

#[test]
fn onto_this_computer_is_at_once_with_a_delete_on_the_server() {
    let mut r = Rig::new();
    let di = r.called("Di");
    let moved = r.mv(&[di], 0)["moved"][0]["id"].as_u64().unwrap();
    let c = r.s.mailbox.contact(moved).unwrap();
    assert!(c.folder == 0 && c.origin.is_local() && c.moved_from == 0, "{c:?}");
    assert_eq!(r.due(r.ada), [(ContactChangeKind::Delete, "c-di".to_string())]);
    // A refusal there leaves it in both places, never in neither.
    let op = r.upload(r.ada).ops.remove(0);
    r.answer(r.ada, &op, ContactResult::Refused, "");
    assert_eq!(r.places("Di"), vec![0, r.team()]);
}

#[test]
fn a_create_that_fails_keeps_the_delete_waiting_and_a_download_that_shows_it_lets_it_go() {
    let mut r = Rig::new();
    let bo = r.called("Bo");
    let eve_main = r.main(r.eve);
    r.mv(&[bo], eve_main);
    let op = r.upload(r.eve).ops.remove(0);
    for _ in 0..3 {
        r.answer(r.eve, &op, ContactResult::Failed, "");
        assert!(r.due(r.ada).is_empty(), "held while Eve's server hasn't made it");
    }
    // Made after all (the answer was lost): a download of Eve's folder shows it.
    let made = RemoteContact { id: "e-bo".into(), change_key: "ck-e-bo".into(), ..op.values.clone() };
    run(&mut r.s, "sync.applyContacts", json!({"download": download(r.eve, EVE, vec![person("e-fay", "Fay"), made])}));
    assert_eq!(r.places("Bo"), vec![eve_main]);
    assert_eq!(r.due(r.ada), [(ContactChangeKind::Delete, "c-bo".to_string())]);
}

#[test]
fn a_create_made_without_a_name_keeps_the_delete_waiting() {
    let mut r = Rig::new();
    let bo = r.called("Bo");
    let eve_main = r.main(r.eve);
    r.mv(&[bo], eve_main);
    let op = r.upload(r.eve).ops.remove(0);
    r.answer(r.eve, &op, ContactResult::Done, "");
    assert!(r.due(r.ada).is_empty());
}

#[test]
fn a_refused_create_puts_the_contact_back_where_it_was_with_its_edits() {
    let mut r = Rig::new();
    let bo = r.called("Bo");
    let (ada_main, eve_main) = (r.main(r.ada), r.main(r.eve));
    let moved = r.mv(&[bo], eve_main)["moved"][0]["id"].as_u64().unwrap();
    run(&mut r.s, "people.edit", json!({"id": moved, "company": "Example Ltd"}));
    let op = r.upload(r.eve).ops.remove(0);
    let said = r.answer(r.eve, &op, ContactResult::Refused, "");
    assert_eq!(said["messages"].as_array().unwrap().len(), 1, "{said}");
    assert!(said["messages"][0].as_str().unwrap().contains("couldn't be moved"), "{said}");
    // Back in Ada's folder as her server's copy, the edit going up as a change there.
    assert_eq!(r.places("Bo"), vec![ada_main]);
    let c = r.s.mailbox.contact(moved).unwrap();
    assert_eq!((c.origin.id.as_str(), c.origin.account, c.company.as_str(), c.moved_from), ("c-bo", r.ada, "Example Ltd", 0));
    assert_eq!(r.due(r.ada), [(ContactChangeKind::Change, "c-bo".to_string())]);
    assert!(r.due(r.eve).is_empty());
    assert!(r.s.mailbox.clone().repair().is_empty());
}

#[test]
fn undo_before_anything_went_up_puts_it_back_and_tells_nobody() {
    let mut r = Rig::new();
    let bo = r.called("Bo");
    let (ada_main, eve_main) = (r.main(r.ada), r.main(r.eve));
    r.mv(&[bo], eve_main);
    run(&mut r.s, "edit.undo", json!({}));
    assert_eq!(r.places("Bo"), vec![ada_main]);
    assert!(r.s.mailbox.contact(bo).is_some_and(|c| c.origin.id == "c-bo"));
    assert!(r.s.mailbox.contact_changes.is_empty());
    // Redone: moved again, the delete held again.
    run(&mut r.s, "edit.redo", json!({}));
    assert_eq!(r.places("Bo"), vec![eve_main]);
    assert!(r.due(r.ada).is_empty());
    assert_eq!(r.due(r.eve).len(), 1);
}

#[test]
fn undo_while_the_create_is_on_its_way_deletes_it_there_once_made() {
    let mut r = Rig::new();
    let bo = r.called("Bo");
    let eve_main = r.main(r.eve);
    r.mv(&[bo], eve_main);
    let op = r.upload(r.eve).ops.remove(0);
    run(&mut r.s, "edit.undo", json!({}));
    assert!(r.due(r.ada).is_empty(), "Bo is back in Ada's folder as her server has him");
    r.answer(r.eve, &op, ContactResult::Done, "e-bo");
    assert_eq!(r.due(r.eve), [(ContactChangeKind::Delete, "e-bo".to_string())]);
    assert!(r.due(r.ada).is_empty());
}

#[test]
fn undo_after_the_create_and_before_the_delete_deletes_the_new_one_only() {
    let mut r = Rig::new();
    let bo = r.called("Bo");
    let (ada_main, eve_main) = (r.main(r.ada), r.main(r.eve));
    r.mv(&[bo], eve_main);
    let op = r.upload(r.eve).ops.remove(0);
    r.answer(r.eve, &op, ContactResult::Done, "e-bo");
    run(&mut r.s, "edit.undo", json!({}));
    assert_eq!(r.places("Bo"), vec![ada_main]);
    assert!(r.due(r.ada).is_empty());
    assert_eq!(r.due(r.eve), [(ContactChangeKind::Delete, "e-bo".to_string())]);
}

#[test]
fn undo_after_both_went_up_sends_the_opposite_of_each() {
    let mut r = Rig::new();
    let bo = r.called("Bo");
    let (ada_main, eve_main) = (r.main(r.ada), r.main(r.eve));
    r.mv(&[bo], eve_main);
    let op = r.upload(r.eve).ops.remove(0);
    r.answer(r.eve, &op, ContactResult::Done, "e-bo");
    let op = r.upload(r.ada).ops.remove(0);
    r.answer(r.ada, &op, ContactResult::Done, "");
    run(&mut r.s, "edit.undo", json!({}));
    assert_eq!(r.places("Bo"), vec![ada_main]);
    assert_eq!(r.due(r.ada), [(ContactChangeKind::Create, String::new())]);
    // Eve's copy goes once Ada's server has made Bo again.
    assert!(r.due(r.eve).is_empty());
    let op = r.upload(r.ada).ops.remove(0);
    r.answer(r.ada, &op, ContactResult::Done, "c-bo-2");
    assert_eq!(r.due(r.eve), [(ContactChangeKind::Delete, "e-bo".to_string())]);
}

#[test]
fn a_folder_that_may_only_be_read_is_refused_either_way_and_nothing_changes() {
    let mut r = Rig::new();
    run(&mut r.s, "sync.applyContacts", json!({"download": team(r.ada, false, vec![person("c-di", "Di")])}));
    let (bo, di, team, eve_main) = (r.called("Bo"), r.called("Di"), r.team(), r.main(r.eve));
    let snapshot = r.s.mailbox.clone();
    for (ids, folder, why) in [
        (vec![di], eve_main, "can't be moved out"),
        (vec![bo], team, "nothing can be moved into it"),
        // One that can't go stops them all.
        (vec![bo, di], eve_main, "can't be moved out"),
    ] {
        let said = r.s.run("people.moveToFolder", &json!({"ids": ids, "folder": folder})).unwrap_err().to_string();
        assert!(said.contains("\"Team\" can only be read") && said.contains(why), "{said}");
        assert!(r.s.mailbox == snapshot);
    }
    for junk in
        [json!({"ids": [bo], "folder": 424_242}), json!({"ids": [], "folder": 0}), json!({"ids": [bo]}), json!({"ids": [999_999], "folder": 0})]
    {
        assert!(r.s.run("people.moveToFolder", &junk).is_err(), "{junk}");
    }
    assert!(r.s.mailbox == snapshot);
}

#[test]
fn a_selection_moves_together_as_one_undo_step() {
    let mut r = Rig::new();
    let gus = run(&mut r.s, "people.new", json!({"first": "Gus", "folder": 0}))["id"].as_u64().unwrap();
    let (bo, cy, eve_main) = (r.called("Bo"), r.called("Cy"), r.main(r.eve));
    let said = r.mv(&[bo, cy, gus, bo], eve_main);
    assert_eq!(said["moved"].as_array().unwrap().len(), 3, "each once: {said}");
    for first in ["Bo", "Cy", "Gus"] {
        assert_eq!(r.places(first), vec![eve_main], "{first}");
    }
    assert_eq!(r.due(r.eve).len(), 3);
    run(&mut r.s, "edit.undo", json!({}));
    assert_eq!((r.folder_of("Bo"), r.folder_of("Gus")), (r.main(r.ada), 0));
    assert!(r.s.mailbox.contact_changes.is_empty());
}

#[test]
fn moving_into_the_folder_it_is_in_changes_nothing() {
    let mut r = Rig::new();
    let (bo, main) = (r.called("Bo"), r.main(r.ada));
    let snapshot = r.s.mailbox.clone();
    assert_eq!(r.mv(&[bo], main)["moved"], json!([]));
    assert!(r.s.mailbox == snapshot);
}

#[test]
fn a_move_waiting_on_its_create_outlives_a_restart() {
    let mut r = Rig::new();
    let bo = r.called("Bo");
    let eve_main = r.main(r.eve);
    r.mv(&[bo], eve_main);
    let text = serde_json::to_string(&r.s.mailbox).unwrap();
    let back: mailcraft_model::Mailbox = serde_json::from_str(&text).unwrap();
    r.s.mailbox = back;
    assert!(r.due(r.ada).is_empty());
}

#[test]
fn the_dialogs_duplicate_check_finds_the_same_address_in_the_target() {
    let mut r = Rig::new();
    let bo = r.called("Bo");
    let eve_main = r.main(r.eve);
    assert_eq!(super::already_in(&r.s.mailbox, &[bo], eve_main), None);
    run(
        &mut r.s,
        "sync.applyContacts",
        json!({"download": download(r.eve, EVE, vec![person("e-fay", "Fay"), RemoteContact { emails: vec![Labeled::new("home", " BO@example.org ")], ..person("e-bob", "Bob") }])}),
    );
    let bob = r.called("Bob");
    assert_eq!(super::already_in(&r.s.mailbox, &[bo], eve_main), Some((bo, bob)));
    // Not in another folder; and a programmatic move doesn't ask.
    assert_eq!(super::already_in(&r.s.mailbox, &[bo], r.team()), None);
    r.mv(&[bo], eve_main);
    assert_eq!(r.places("Bo"), vec![eve_main]);
}

#[test]
fn a_contact_still_being_made_on_a_server_isnt_moved_again() {
    let mut r = Rig::new();
    let (bo, team, ada_main, eve_main) = (r.called("Bo"), r.team(), r.main(r.ada), r.main(r.eve));
    // Moved once: the new one waits to be made on Eve's server, Ada's copy held for it.
    let moved = r.mv(&[bo], eve_main)["moved"][0]["id"].as_u64().unwrap();
    let before = r.s.mailbox.clone();
    let e = r.s.run("people.moveToFolder", &json!({"ids": [moved], "folder": team})).unwrap_err();
    assert!(e.to_string().contains("still being sent"), "{e}");
    assert!(r.s.mailbox == before, "nothing changed");
    // A new contact whose Create may be in the sync thread's batch already, not answered yet.
    let gus = run(&mut r.s, "people.new", json!({"first": "Gus", "folder": ada_main}))["id"].as_u64().unwrap();
    let _in_flight = r.upload(r.ada);
    assert!(r.s.run("people.moveToFolder", &json!({"ids": [gus], "folder": eve_main})).is_err());
    assert_eq!(r.places("Gus"), vec![ada_main]);
    // Once made, it moves.
    let op = r.upload(r.eve).ops.remove(0);
    r.answer(r.eve, &op, ContactResult::Done, "e-bo");
    assert!(r.s.run("people.moveToFolder", &json!({"ids": [moved], "folder": team})).is_ok());
}

#[test]
fn undoing_a_later_edit_after_a_refused_create_doesnt_send_it_again() {
    let mut r = Rig::new();
    let gus = run(&mut r.s, "people.new", json!({"first": "Gus", "folder": 0}))["id"].as_u64().unwrap();
    let ada_main = r.main(r.ada);
    r.mv(&[gus], ada_main);
    run(&mut r.s, "people.edit", json!({"id": gus, "company": "Example Ltd"}));
    let op = r.upload(r.ada).ops.remove(0);
    r.answer(r.ada, &op, ContactResult::Refused, "");
    let kept = r.folder_of("Gus");
    run(&mut r.s, "edit.undo", json!({}));
    assert_eq!(r.folder_of("Gus"), kept, "the undone edit leaves the folder the refusal gave it");
    assert!(r.due(r.ada).is_empty(), "the refused Create isn't sent again");
}

#[test]
fn undoing_a_later_edit_after_a_move_went_back_tells_only_the_server_that_has_it() {
    let mut r = Rig::new();
    let bo = r.called("Bo");
    let (ada_main, eve_main) = (r.main(r.ada), r.main(r.eve));
    let moved = r.mv(&[bo], eve_main)["moved"][0]["id"].as_u64().unwrap();
    run(&mut r.s, "people.edit", json!({"id": moved, "company": "Example Ltd"}));
    let op = r.upload(r.eve).ops.remove(0);
    r.answer(r.eve, &op, ContactResult::Refused, "");
    assert_eq!(r.places("Bo"), vec![ada_main]);
    run(&mut r.s, "edit.undo", json!({}));
    assert_eq!(r.places("Bo"), vec![ada_main]);
    let c = r.s.mailbox.contact(moved).unwrap();
    assert_eq!((c.origin.account, c.origin.id.as_str(), c.company.as_str()), (r.ada, "c-bo", ""));
    assert!(r.due(r.eve).is_empty(), "Eve's server is never sent Ada's id");
}

#[test]
fn undo_after_both_went_up_holds_the_delete_for_the_create_again() {
    let mut r = Rig::new();
    let bo = r.called("Bo");
    let eve_main = r.main(r.eve);
    r.mv(&[bo], eve_main);
    let op = r.upload(r.eve).ops.remove(0);
    r.answer(r.eve, &op, ContactResult::Done, "e-bo");
    let op = r.upload(r.ada).ops.remove(0);
    r.answer(r.ada, &op, ContactResult::Done, "");
    run(&mut r.s, "edit.undo", json!({}));
    assert!(r.due(r.eve).is_empty(), "Eve's copy waits for Ada's server to make Bo again");
    let op = r.upload(r.ada).ops.remove(0);
    r.answer(r.ada, &op, ContactResult::Refused, "");
    // Refused: Bo stays in Eve's folder, never in neither.
    assert_eq!(r.places("Bo"), vec![eve_main]);
    assert!(r.due(r.eve).is_empty());
}

#[test]
fn repair_drops_a_moves_delete_with_its_create_and_a_link_to_nothing() {
    let mut r = Rig::new();
    let (bo, cy, eve_main) = (r.called("Bo"), r.called("Cy"), r.main(r.eve));
    run(&mut r.s, "people.edit", json!({"id": cy, "last": "Lee"}));
    let moved = r.mv(&[bo], eve_main)["moved"][0]["id"].as_u64().unwrap();
    let mut m = r.s.mailbox.clone();
    // Cy names a contact with no deletion waiting: the link goes.
    m.contact_mut(cy).unwrap().moved_from = 999;
    // The Create is cut by the cap (it is last): its held Delete goes with it, never sent alone.
    let take = |m: &mut mailcraft_model::Mailbox, id: Id| {
        let at = m.contact_changes.iter().position(|n| n.contact == id).unwrap();
        m.contact_changes.remove(at)
    };
    let (delete, create, change) = (take(&mut m, bo), take(&mut m, moved), take(&mut m, cy));
    m.contact_changes = vec![delete; 1];
    while m.contact_changes.len() < mailcraft_model::contact_sync::MAX_CONTACT_CHANGES {
        m.contact_changes.push(change.clone());
    }
    m.contact_changes.push(create);
    m.repair();
    assert_eq!(m.contact(cy).unwrap().moved_from, 0);
    assert!(!m.contact_changes.iter().any(|n| n.contact == moved), "the Create was cut");
    assert!(!m.contact_changes.iter().any(|n| n.contact == bo), "so its Delete is dropped too");
}

// Re-review of #1944 (CS5.1).

#[test]
fn one_still_being_made_moves_onto_this_computer_and_a_late_done_deletes_it_there() {
    let mut r = Rig::new();
    let ada_main = r.main(r.ada);
    let gus = run(&mut r.s, "people.new", json!({"first": "Gus", "folder": ada_main}))["id"].as_u64().unwrap();
    // Its Create is on its way, unanswered.
    let op = r.upload(r.ada).ops.remove(0);
    let here = r.mv(&[gus], 0)["moved"][0]["id"].as_u64().unwrap();
    assert_ne!(here, gus, "a late answer is about the id it leaves behind");
    assert_eq!(r.places("Gus"), vec![0]);
    assert!(r.due(r.ada).is_empty(), "nothing to make there now");
    // The server made it after all: it is told to delete it, and Gus stays here.
    r.answer(r.ada, &op, ContactResult::Done, "c-gus");
    assert_eq!(r.due(r.ada), [(ContactChangeKind::Delete, "c-gus".to_string())]);
    let op = r.upload(r.ada).ops.remove(0);
    r.answer(r.ada, &op, ContactResult::Done, "");
    assert_eq!(r.places("Gus"), vec![0]);
    assert!(r.s.mailbox.contact_changes.is_empty());
}

/// A delta read of Ada's folder, `days` after NOW, that brings `contacts` (not a whole list).
fn delta(r: &Rig, days: i64, contacts: Vec<RemoteContact>) -> ContactsDownload {
    ContactsDownload { complete: false, read_at: NOW + days * 86_400, ..download(r.ada, ADA, contacts) }
}

/// Independent review of #1951 (P1): a move onto this computer leaves no record behind that could
/// be taken for a contact made on the server later, nor keep the account's contacts due.
#[test]
fn moved_onto_this_computer_before_it_went_up_leaves_nothing_behind() {
    let mut r = Rig::new();
    let ada_main = r.main(r.ada);
    let gus = run(&mut r.s, "people.new", json!({"first": "Gus", "email": "gus@example.org", "folder": ada_main}))["id"].as_u64().unwrap();
    r.mv(&[gus], 0);
    assert!(r.s.mailbox.contact_changes.is_empty(), "{:?}", r.s.mailbox.contact_changes);
    assert!(!crate::cmd::contacts_remote::contacts_due(&r.s.mailbox, r.ada, r.s.now()), "polling back to its own pace");
    // Weeks later the same Gus is added on a phone: the next delta takes him in, and deletes nothing.
    let phone = RemoteContact { emails: vec![Labeled::new("work", "gus@example.org")], ..person("c-gus-phone", "Gus") };
    let later = delta(&r, 21, vec![phone]);
    run(&mut r.s, "sync.applyContacts", json!({"download": later}));
    assert_eq!(r.places("Gus"), vec![0, ada_main]);
    assert!(r.due(r.ada).is_empty());
}

#[test]
fn moved_onto_this_computer_while_being_sent_a_late_failure_leaves_nothing_behind() {
    let mut r = Rig::new();
    let ada_main = r.main(r.ada);
    let gus = run(&mut r.s, "people.new", json!({"first": "Gus", "email": "gus@example.org", "folder": ada_main}))["id"].as_u64().unwrap();
    let op = r.upload(r.ada).ops.remove(0);
    r.mv(&[gus], 0);
    // lean: a copy the server made despite the failure comes down as a second Gus (never lost).
    r.answer(r.ada, &op, ContactResult::Failed, "");
    assert!(r.s.mailbox.contact_changes.is_empty(), "{:?}", r.s.mailbox.contact_changes);
    assert!(!crate::cmd::contacts_remote::contacts_due(&r.s.mailbox, r.ada, r.s.now()));
    let phone = RemoteContact { emails: vec![Labeled::new("work", "gus@example.org")], ..person("c-gus-phone", "Gus") };
    let later = delta(&r, 21, vec![phone]);
    run(&mut r.s, "sync.applyContacts", json!({"download": later}));
    assert_eq!(r.places("Gus"), vec![0, ada_main]);
    assert!(r.due(r.ada).is_empty());
}

#[test]
fn moved_on_again_before_the_late_answer_it_is_made_where_it_went_and_deleted_where_it_was_sent() {
    let mut r = Rig::new();
    let (ada_main, team) = (r.main(r.ada), r.team());
    let gus = run(&mut r.s, "people.new", json!({"first": "Gus", "folder": ada_main}))["id"].as_u64().unwrap();
    let op = r.upload(r.ada).ops.remove(0);
    let here = r.mv(&[gus], 0)["moved"][0]["id"].as_u64().unwrap();
    r.mv(&[here], team);
    r.answer(r.ada, &op, ContactResult::Done, "c-gus");
    assert_eq!(r.places("Gus"), vec![team]);
    let mut due = r.due(r.ada);
    due.sort_unstable_by_key(|(_, remote)| remote.clone());
    assert_eq!(due, [(ContactChangeKind::Create, String::new()), (ContactChangeKind::Delete, "c-gus".to_string())]);
    let delete = r.s.mailbox.contact_changes.iter().find(|n| n.kind == ContactChangeKind::Delete).unwrap();
    assert_eq!(delete.folder, ada_main, "deleted from the folder it was sent to");
}

#[test]
fn undo_of_a_move_onto_this_computer_while_being_sent_takes_the_late_answer_as_it_was() {
    let mut r = Rig::new();
    let ada_main = r.main(r.ada);
    let gus = run(&mut r.s, "people.new", json!({"first": "Gus", "folder": ada_main}))["id"].as_u64().unwrap();
    let op = r.upload(r.ada).ops.remove(0);
    r.mv(&[gus], 0);
    run(&mut r.s, "edit.undo", json!({}));
    assert_eq!(r.places("Gus"), vec![ada_main]);
    r.answer(r.ada, &op, ContactResult::Done, "c-gus");
    assert_eq!(r.s.mailbox.contact(gus).unwrap().origin.id, "c-gus");
    assert!(r.s.mailbox.contact_changes.is_empty(), "{:?}", r.s.mailbox.contact_changes);
}

#[test]
fn a_move_still_being_made_goes_onto_this_computer_and_both_servers_copies_go() {
    let mut r = Rig::new();
    let (bo, eve_main) = (r.called("Bo"), r.main(r.eve));
    let moved = r.mv(&[bo], eve_main)["moved"][0]["id"].as_u64().unwrap();
    let op = r.upload(r.eve).ops.remove(0);
    r.mv(&[moved], 0);
    assert_eq!(r.places("Bo"), vec![0]);
    // Kept here: Ada's copy goes now, Eve's once her server answers.
    assert_eq!(r.due(r.ada), [(ContactChangeKind::Delete, "c-bo".to_string())]);
    r.answer(r.eve, &op, ContactResult::Done, "e-bo");
    assert_eq!(r.due(r.eve), [(ContactChangeKind::Delete, "e-bo".to_string())]);
    assert_eq!(r.places("Bo"), vec![0]);
}

#[test]
fn one_that_may_be_on_its_server_unheard_isnt_moved_onto_this_computer() {
    let mut r = Rig::new();
    let ada_main = r.main(r.ada);
    let gus = run(&mut r.s, "people.new", json!({"first": "Gus", "folder": ada_main}))["id"].as_u64().unwrap();
    let op = r.upload(r.ada).ops.remove(0);
    // Failed: it may have been made, and only a later answer or a download will say.
    r.answer(r.ada, &op, ContactResult::Failed, "");
    let before = r.s.mailbox.clone();
    let e = r.s.run("people.moveToFolder", &json!({"ids": [gus], "folder": 0})).unwrap_err();
    assert!(e.to_string().contains("still being sent"), "{e}");
    assert!(r.s.mailbox == before);
}

#[test]
fn the_names_of_those_that_wait_are_cleaned_and_capped() {
    let mut r = Rig::new();
    let (ada_main, eve_main) = (r.main(r.ada), r.main(r.eve));
    let long = format!("Gus\u{7}{}", "x".repeat(500));
    let ids: Vec<Id> = (0..8)
        .map(|n| {
            run(&mut r.s, "people.new", json!({"first": if n == 0 { long.clone() } else { format!("P{n}") }, "folder": ada_main}))["id"]
                .as_u64()
                .unwrap()
        })
        .collect();
    let e = r.s.run("people.moveToFolder", &json!({"ids": ids, "folder": eve_main})).unwrap_err().to_string();
    assert!(e.starts_with("8 contacts are still being sent") && e.contains("P4") && !e.contains("P5"), "{e}");
    assert!(!e.contains('\u{7}') && e.chars().count() < 300, "{e}");
}

#[test]
fn a_selection_with_one_still_being_made_moves_the_others_and_names_it() {
    let mut r = Rig::new();
    let (bo, ada_main, eve_main) = (r.called("Bo"), r.main(r.ada), r.main(r.eve));
    let gus = run(&mut r.s, "people.new", json!({"first": "Gus", "folder": ada_main}))["id"].as_u64().unwrap();
    let said = r.mv(&[bo, gus], eve_main);
    assert_eq!(said["moved"].as_array().unwrap().len(), 1, "{said}");
    assert_eq!(said["skipped"], json!([{"contact": gus, "name": "\"Gus\""}]));
    assert_eq!((r.places("Bo"), r.places("Gus")), (vec![eve_main], vec![ada_main]));
}

#[test]
fn undo_while_the_sources_delete_is_on_its_way_holds_the_new_ones_delete_until_it_is_made_again() {
    let mut r = Rig::new();
    let bo = r.called("Bo");
    let eve_main = r.main(r.eve);
    r.mv(&[bo], eve_main);
    let op = r.upload(r.eve).ops.remove(0);
    r.answer(r.eve, &op, ContactResult::Done, "e-bo");
    // Ada's server is told to delete Bo; undone before it answers.
    let delete = r.upload(r.ada).ops.remove(0);
    assert_eq!(delete.kind, ContactChangeKind::Delete);
    run(&mut r.s, "edit.undo", json!({}));
    // Deleted there after all: Bo is made there again, and Eve's copy waits for that.
    r.answer(r.ada, &delete, ContactResult::Done, "");
    assert_eq!(r.due(r.ada), [(ContactChangeKind::Create, String::new())]);
    assert!(r.due(r.eve).is_empty(), "Eve's copy waits for Ada's server to make Bo again");
    // Ada's server won't: Bo stays in Eve's folder, never in neither.
    let op = r.upload(r.ada).ops.remove(0);
    r.answer(r.ada, &op, ContactResult::Refused, "");
    assert_eq!(r.places("Bo"), vec![eve_main]);
    assert!(r.due(r.eve).is_empty());
}
