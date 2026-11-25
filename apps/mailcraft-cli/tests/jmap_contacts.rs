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
