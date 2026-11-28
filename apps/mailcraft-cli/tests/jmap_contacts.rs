// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A JMAP account's contacts (CS10, #1068, #1076) from end to end: the engine's request and
//! `apply_jmap_mail`, with `mailcraft_jmap`'s run in between, against the crate's JMAP server in the
//! process (`stand-in`). Down: address books with their rights and owners, then only what changed. Up:
//! a new contact, a change of one field (nothing else of the card written), a deletion. Nothing here
//! leaves the process; every person is invented.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::atomic::AtomicBool;

use mailcraft_engine::Session;
use mailcraft_engine::cmd::jmap_mail::{apply_jmap_mail, jmap_mail_request};
use mailcraft_jmap::standin::{ACCOUNT, SESSION, StandIn, TOKEN};
use mailcraft_jmap::{Connection, Credentials, block_on, sync_mail};
use mailcraft_model::contact_sync::FolderOwner;
use mailcraft_model::{Contact, Id, Mailbox};
use serde_json::{Value, json};

const ME: &str = "ada@example.org";
const ZOE: &str = "zoe@example.org";

struct Rig {
    server: StandIn,
    s: Session,
    account: Id,
}

impl Rig {
    fn new() -> Rig {
        let server = StandIn::new();
        server.with(|d| {
            d.mailbox("Inbox", None, Some("inbox"));
            d.contacts.on = true;
            d.contacts.shared = Some(("A2".into(), ZOE.into()));
            let personal = d.contacts.book(ACCOUNT, "Personal", true);
            let zoes = d.contacts.book("A2", "Zoe's people", true);
            if let Some(b) = d.contacts.books.iter_mut().find(|b| b.id == zoes) {
                b.may_write = false;
            }
            d.contacts.card(
                ACCOUNT,
                json!({"uid": "urn:uuid:bo", "addressBookIds": {personal: true}, "name": {"full": "Bo Park"},
                    "emails": {"e1": {"address": "bo@example.org"}}, "imAddresses": {"i1": {"handle": "bo"}}}),
            );
            d.contacts.card("A2", json!({"uid": "urn:uuid:di", "addressBookIds": {zoes: true}, "name": {"full": "Di Zoe"}}));
        });
        let mut s = Session::at(Mailbox::with_local_account("", ""), 1_800_000_000, 0);
        let added = s.run("account.add", &json!({"email": ME, "kind": "jmap", "jmap": {"session": SESSION, "bearer": true}})).unwrap();
        Rig { server, s, account: added["id"].as_u64().unwrap() }
    }

    fn run(&mut self) -> Vec<Value> {
        let request = jmap_mail_request(&self.s, self.account, true).unwrap();
        let connection = block_on(Connection::connect(self.server.clone(), SESSION, Credentials::bearer(TOKEN).unwrap())).unwrap();
        let mut pieces = Vec::new();
        block_on(sync_mail(&connection, &request, &AtomicBool::new(false), &mut |piece| pieces.push(piece)));
        pieces.into_iter().map(|piece| apply_jmap_mail(&mut self.s, piece).unwrap()).collect()
    }

    fn contact(&self, name: &str) -> Option<&Contact> {
        self.s.mailbox.contacts.values().map(|c| &**c).find(|c| c.name() == name)
    }

    fn server_card(&self, name: &str) -> Option<(String, Value)> {
        self.server.with(|d| d.contacts.named(name).map(|(id, c)| (id.clone(), c.clone())))
    }
}

#[test]
fn a_jmap_accounts_contacts_come_down_and_what_is_made_changed_and_deleted_here_goes_up() {
    let mut rig = Rig::new();
    let answers = rig.run();
    assert_eq!(answers.last().unwrap()["errors"], json!([]), "{answers:?}");

    // Down: the own book is the account's main folder; Zoe's is hers and can only be read.
    let bo = rig.contact("Bo Park").cloned().unwrap();
    let main = rig.s.mailbox.contact_folder(bo.folder).cloned().unwrap();
    assert!(main.is_main() && main.writable && main.account == rig.account && !main.delta.is_empty());
    let di = rig.contact("Di Zoe").cloned().unwrap();
    let zoes = rig.s.mailbox.contact_folder(di.folder).cloned().unwrap();
    assert_eq!((zoes.owner.clone(), zoes.writable), (FolderOwner::Person(ZOE.into()), false));
    assert!(rig.s.run("people.edit", &json!({"id": di.id, "notes": "x"})).is_err(), "a read-only folder refuses a change");

    // Up: a new contact goes to the account's main folder, and to the server's default book.
    let made = rig.s.run("people.new", &json!({"first": "Ed", "last": "New", "email": "ed@example.org"})).unwrap();
    assert_eq!(made["folder"].as_u64(), Some(main.id));
    let ed = made["id"].as_u64().unwrap();
    rig.run();
    let (ed_remote, card) = rig
        .server
        .with(|d| {
            d.contacts
                .cards
                .iter()
                .find(|(_, (_, c))| c.pointer("/emails").is_some_and(|e| e.to_string().contains("ed@example.org")))
                .map(|(id, (_, c))| (id.clone(), c.clone()))
        })
        .unwrap();
    assert!(card["name"]["components"].to_string().contains("Ed"));
    assert_eq!(rig.s.mailbox.contact(ed).unwrap().origin.id, ed_remote);
    assert_eq!(rig.s.run("people.get", &json!({"id": ed})).unwrap()["notSent"], json!(false));

    // A change of one field patches that property alone: what MailCraft doesn't show stays.
    rig.s.run("people.edit", &json!({"id": bo.id, "notes": "Met at the fair."})).unwrap();
    rig.run();
    let (bo_remote, card) = rig.server_card("Bo Park").unwrap();
    assert_eq!(card["notes"], json!({"n1": {"note": "Met at the fair."}}));
    assert_eq!(card["imAddresses"], json!({"i1": {"handle": "bo"}}));
    assert_eq!(
        rig.server.with(|d| d.contacts.patches.last().cloned()),
        Some((bo_remote.clone(), json!({"notes": {"n1": {"note": "Met at the fair."}}})))
    );

    // A change made elsewhere comes down from where the last read left off.
    rig.server.with(|d| d.contacts.change(&bo_remote, "phones", json!({"p1": {"number": "+44 20 7946 0009"}})));
    let before = rig.server.calls().len();
    rig.run();
    let asked: Vec<String> = rig.server.calls().into_iter().skip(before).collect();
    assert!(asked.contains(&"ContactCard/changes".to_string()) && !asked.contains(&"ContactCard/query".to_string()), "{asked:?}");
    let bo_here = rig.contact("Bo Park").unwrap();
    assert_eq!(bo_here.phones.first().map(|p| p.value.as_str()), Some("+44 20 7946 0009"));
    assert_eq!(bo_here.notes, "Met at the fair.");

    // A deletion here destroys the card.
    rig.s.run("people.delete", &json!({"id": ed})).unwrap();
    rig.run();
    assert!(rig.server.with(|d| d.contacts.get(&ed_remote).is_none()));
    assert!(rig.s.mailbox.contact_changes.is_empty(), "{:?}", rig.s.mailbox.contact_changes);
}

impl Rig {
    /// A second book of the account's own, "Work": its id.
    fn work_book(&self) -> String {
        self.server.with(|d| d.contacts.book(ACCOUNT, "Work", false))
    }

    fn folder_named(&self, name: &str) -> Option<Id> {
        self.s.mailbox.contact_folders.iter().find(|f| f.name == name).map(|f| f.id)
    }
}

#[test]
fn a_new_contact_whose_answer_was_lost_makes_the_next_run_read_its_book_whole() {
    let mut rig = Rig::new();
    rig.run();
    let own = |s: &Session, account: Id| {
        let request = jmap_mail_request(s, account, true).unwrap();
        request.contacts.unwrap().places.into_iter().find(|p| p.owner == FolderOwner::Own).unwrap().state
    };
    assert!(!own(&rig.s, rig.account).is_empty(), "read up to a state");
    let main = rig.s.mailbox.contact_folders.iter().find(|f| f.account == rig.account && f.is_main()).unwrap().id;
    rig.s.mailbox.contact_changes.push(mailcraft_model::contact_sync::ContactChange {
        account: rig.account,
        kind: mailcraft_model::contact_sync::ContactChangeKind::Create,
        contact: 999,
        folder: main,
        sent: true,
        unconfirmed: 1,
        ..Default::default()
    });
    assert_eq!(own(&rig.s, rig.account), "", "read whole, so the card is found or known not made");
    rig.s.mailbox.contact_changes.retain(|c| c.contact != 999);

    // A book whose whole read never completes: the new contact waits a day, then goes up again.
    let made = rig.s.run("people.new", &json!({"first": "Lu", "last": "Lost"})).unwrap()["id"].as_u64().unwrap();
    let mark = |s: &mut Session, at: i64| {
        let change = s.mailbox.contact_changes.iter_mut().find(|c| c.contact == made).unwrap();
        (change.sent, change.unconfirmed) = (true, at);
    };
    let unconfirmed = |s: &Session| s.mailbox.contact_changes.iter().find(|c| c.contact == made).map(|c| c.unconfirmed);
    rig.server.with(|d| d.contacts.query_moves = true);
    let now = rig.s.now();
    mark(&mut rig.s, now - 10);
    rig.run();
    assert_eq!(unconfirmed(&rig.s), Some(now - 10), "still waiting");
    assert!(!rig.server.with(|d| d.contacts.cards.values().any(|(_, c)| c.to_string().contains("Lost"))), "not made twice");
    mark(&mut rig.s, now - 2 * 86_400);
    rig.run();
    assert_eq!(unconfirmed(&rig.s), Some(0), "a day on, it goes up again");
}

#[test]
fn deleting_a_contact_from_one_book_takes_the_card_out_of_that_book_only() {
    let mut rig = Rig::new();
    let work = rig.work_book();
    let personal = rig.server.with(|d| d.contacts.books.iter().find(|b| b.default && b.account == ACCOUNT).map(|b| b.id.clone())).unwrap();
    let both = rig.server.with(|d| {
        d.contacts.card(
            ACCOUNT,
            json!({"uid": "urn:uuid:fay", "addressBookIds": {personal.clone(): true, work.clone(): true}, "name": {"full": "Fay Both"}}),
        )
    });
    rig.run();
    let work_folder = rig.folder_named("Work").unwrap();
    let copies: Vec<(Id, Id)> = rig.s.mailbox.contacts.values().filter(|c| c.name() == "Fay Both").map(|c| (c.id, c.folder)).collect();
    assert_eq!(copies.len(), 2, "one in each folder: {copies:?}");
    let (in_work, _) = copies.iter().copied().find(|(_, f)| *f == work_folder).unwrap();

    // Deleted from Work: the card stays, in Personal alone.
    rig.s.run("people.delete", &json!({"id": in_work})).unwrap();
    rig.run();
    let card = rig.server.with(|d| d.contacts.get(&both).cloned()).expect("not destroyed");
    assert_eq!(card["addressBookIds"], json!({personal.clone(): true}));
    assert_eq!(rig.server.with(|d| d.contacts.patches.last().cloned()), Some((both.clone(), json!({format!("addressBookIds/{work}"): null}))));
    let left: Vec<Id> = rig.s.mailbox.contacts.values().filter(|c| c.name() == "Fay Both").map(|c| c.folder).collect();
    assert_eq!(left.len(), 1);
    assert_ne!(left[0], work_folder);
    assert!(rig.s.mailbox.contact_changes.is_empty(), "{:?}", rig.s.mailbox.contact_changes);

    // Deleted from its last book: destroyed.
    let last = rig.contact("Fay Both").unwrap().id;
    rig.s.run("people.delete", &json!({"id": last})).unwrap();
    rig.run();
    assert!(rig.server.with(|d| d.contacts.get(&both).is_none()));
}

#[test]
fn a_book_gone_from_the_server_takes_its_folder_and_keeps_what_waited_to_go_up() {
    let mut rig = Rig::new();
    let work = rig.work_book();
    let cy = rig
        .server
        .with(|d| d.contacts.card(ACCOUNT, json!({"uid": "urn:uuid:cy", "addressBookIds": {work.clone(): true}, "name": {"full": "Cy Work"}})));
    let dee = rig
        .server
        .with(|d| d.contacts.card(ACCOUNT, json!({"uid": "urn:uuid:dee", "addressBookIds": {work.clone(): true}, "name": {"full": "Dee Work"}})));
    rig.run();
    let work_folder = rig.folder_named("Work").unwrap();
    assert_eq!(rig.contact("Cy Work").map(|c| c.folder), Some(work_folder));

    // Cy is changed here; the book goes on the server before that goes up.
    let cy_here = rig.contact("Cy Work").unwrap().id;
    rig.s.run("people.edit", &json!({"id": cy_here, "notes": "Kept."})).unwrap();
    rig.server.with(|d| {
        d.contacts.remove(&cy);
        d.contacts.remove(&dee);
        d.contacts.books.retain(|b| b.id != work);
    });
    let answers = rig.run();

    assert_eq!(rig.folder_named("Work"), None, "the folder went");
    assert!(rig.contact("Dee Work").is_none(), "its contacts went with it");
    let kept = rig.contact("Cy Work").expect("the one with a change waiting is kept");
    assert_eq!((kept.folder, kept.notes.as_str(), kept.origin.id.as_str()), (0, "Kept.", ""));
    assert!(rig.s.mailbox.contact_changes.is_empty(), "{:?}", rig.s.mailbox.contact_changes);
    assert!(json!(answers).to_string().contains("is kept on this computer"), "{answers:?}");
    // The main folder and Bo stay.
    assert!(rig.contact("Bo Park").is_some());
}
