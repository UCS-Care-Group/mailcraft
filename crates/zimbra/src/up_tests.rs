// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Changes up, against the stand-in: folders made, renamed and deleted, messages marked, tagged,
//! moved and deleted, and mail sent once, with its Bcc never written.

use std::sync::atomic::AtomicBool;

use mailcraft_model::sync::{
    CategoriesUp, ChangeOutcome, FlagUp, FolderChange, MessageChange, MessageOutcome, SendOutcome, SendUp, ZimbraMailRequest, ZimbraMailResult,
};
use mailcraft_model::{Address, FlagStatus, Message};

use crate::auth::Credentials;
use crate::standin::{PASSWORD, StandIn, USER, block_on};
use crate::sync_mail;

#[test]
fn a_missing_mutation_target_does_not_prove_the_source_is_gone() {
    assert!(matches!(crate::up::message_refused(crate::Error::NoSuchItem), MessageOutcome::Refused { .. }));
    let change = FolderChange::Create { folder: 9, name: "New".into(), parent: "900".into(), parent_folder: None };
    let response = crate::HttpResponse::new(
        500,
        serde_json::json!({"Body": {"Fault": {"Detail": {"Error": {"Code": "mail.NO_SUCH_FOLDER"}}}}}).to_string().into_bytes(),
    );
    assert!(matches!(crate::up::folder_done(&change, &response), ChangeOutcome::Refused { .. }));
}

fn request() -> ZimbraMailRequest {
    ZimbraMailRequest { account: 3, email: USER.into(), url: "http://127.0.0.1:7070".into(), ..Default::default() }
}

fn run(server: &StandIn, request: &ZimbraMailRequest) -> ZimbraMailResult {
    let mut all = ZimbraMailResult::default();
    block_on(sync_mail(server, request, &Credentials::password(PASSWORD), &AtomicBool::new(false), &mut |_| {}, &mut |piece| {
        assert_eq!(piece.check(), Ok(()), "{piece:?}");
        all.absorb(piece);
    }))
    .unwrap();
    all
}

fn message(subject: &str) -> Message {
    Message {
        id: 77,
        subject: subject.into(),
        from: Address::new("Ada", USER),
        to: vec![Address::new("Bob", "bob@example.org")],
        date: 1_700_000_000,
        ..Default::default()
    }
}

#[test]
fn folders_are_made_renamed_and_deleted_and_the_servers_own_stay() {
    let server = StandIn::new();
    let old = server.add_folder(1, "Old");
    let mut asked = request();
    asked.changes = vec![
        FolderChange::Create { folder: 9, name: "Projects".into(), parent: "2".into(), parent_folder: None },
        FolderChange::Rename { folder: 10, id: old.to_string(), name: "Renamed".into() },
        FolderChange::Delete { id: "2".into(), shown: Default::default() },
        FolderChange::Delete { id: "999".into(), shown: Default::default() },
    ];
    let all = run(&server, &asked);
    assert_eq!(all.errors, Vec::<String>::new());
    let outcomes: Vec<&ChangeOutcome> = all.changes.iter().map(|d| &d.outcome).collect();
    assert!(matches!(outcomes[0], ChangeOutcome::Done { folder } if folder.name == "Projects" && folder.parent == "2" && !folder.id.is_empty()));
    assert!(matches!(outcomes[1], ChangeOutcome::Done { folder } if folder.name == "Renamed"));
    assert!(matches!(outcomes[2], ChangeOutcome::OwnFolder { .. }));
    assert!(matches!(outcomes[3], ChangeOutcome::Refused { .. }));
    let s = server.state.borrow();
    assert!(s.folders.values().any(|(parent, name, _)| *parent == 2 && name == "Projects"));
    assert_eq!(s.folders.get(&old).map(|f| f.1.as_str()), Some("Renamed"));
    // The changes went before the mailbox was read.
    let first_read = s.log.iter().position(|l| l == "GetFolderRequest").unwrap();
    assert!(s.log.iter().position(|l| l == "CreateFolderRequest").unwrap() < first_read);
}

#[test]
fn messages_are_marked_tagged_moved_and_deleted_by_their_ids() {
    let server = StandIn::new();
    let projects = server.add_folder(1, "Projects");
    let marked = server.deliver(2, "Marked", "u");
    let moved = server.deliver(2, "Moved", "");
    let deleted = server.deliver(2, "Deleted", "");
    let mut asked = request();
    let flag = FlagUp { status: FlagStatus::Flagged, ..Default::default() };
    let categories = CategoriesUp { synced: vec![], here: vec!["Work".into(), "Red, or not".into()] };
    asked.messages = vec![
        MessageChange::Update {
            message: 1,
            id: marked.to_string(),
            read: Some(true),
            flag: Some(flag),
            categories: Some(categories),
            importance: None,
        },
        MessageChange::Move { message: 2, id: moved.to_string(), to: projects.to_string(), to_folder: None },
        MessageChange::Delete { id: deleted.to_string() },
        MessageChange::Delete { id: "999".into() },
    ];
    let all = run(&server, &asked);
    assert_eq!(all.errors, Vec::<String>::new());
    let outcomes: Vec<&MessageOutcome> = all.messages.iter().map(|d| &d.outcome).collect();
    assert!(matches!(outcomes[0], MessageOutcome::Done { categories: Some(c), .. } if c.len() == 2));
    assert!(matches!(outcomes[1], MessageOutcome::Done { folder, id, .. } if *folder == projects.to_string() && *id == moved.to_string()));
    assert!(matches!(outcomes[2], MessageOutcome::Gone));
    assert!(matches!(outcomes[3], MessageOutcome::Refused { .. }));
    let s = server.state.borrow();
    let m = s.items.get(&marked).unwrap();
    assert!(!m.flags.contains('u') && m.flags.contains('f'));
    assert_eq!(m.tags, ["Work", "Red  or not"]);
    assert_eq!(s.items.get(&moved).map(|m| m.folder), Some(projects));
    assert!(!s.items.contains_key(&deleted));
}

#[test]
fn a_tag_the_server_doesnt_have_is_made_in_the_colour_it_has_here() {
    let server = StandIn::new();
    server.state.borrow_mut().tags.insert("Known".into(), "#0084C7".into());
    let marked = server.deliver(2, "Marked", "");
    let mut asked = request();
    asked.categories = vec![
        mailcraft_model::Category { name: "Known".into(), color: mailcraft_model::Rgb(0, 0, 0) },
        mailcraft_model::Category { name: "CraftiCraft".into(), color: mailcraft_model::Rgb(0xF5, 0x8E, 0xE1) },
    ];
    asked.messages = vec![MessageChange::Update {
        message: 1,
        id: marked.to_string(),
        read: None,
        flag: None,
        categories: Some(CategoriesUp { synced: vec![], here: vec!["Known".into(), "CraftiCraft".into(), "Colouress".into()] }),
        importance: None,
    }];
    run(&server, &asked);
    let s = server.state.borrow();
    // One CreateTagRequest: for the new name that has a colour here, before the message's update.
    assert_eq!(s.log.iter().filter(|l| *l == "CreateTagRequest").count(), 1);
    let made = s.log.iter().position(|l| *l == "CreateTagRequest").unwrap();
    let update = s.log.iter().position(|l| *l == "MsgActionRequest").unwrap();
    assert!(made < update);
    assert_eq!(s.tags.get("CraftiCraft").map(String::as_str), Some("#F58EE1"));
    // The server's own tag wasn't made again; a name with no colour here went through `tn` alone.
    assert_eq!(s.tags.get("Known").map(String::as_str), Some("#0084C7"));
    assert_eq!(s.tags.get("Colouress").map(String::as_str), Some(""));
    assert_eq!(s.items.get(&marked).unwrap().tags, ["Known", "CraftiCraft", "Colouress"]);
}

#[test]
fn mail_is_sent_once_by_its_upload_and_a_bcc_is_held() {
    let server = StandIn::new();
    let mut asked = request();
    let mut secret = message("With a Bcc");
    secret.id = 78;
    secret.bcc = vec![Address::new("", "hidden@example.org")];
    asked.sends = vec![
        SendUp { message: 77, draft: None, mime: Some(Box::new(message("Hello from MailCraft"))) },
        SendUp { message: 78, draft: None, mime: Some(Box::new(secret)) },
    ];
    let all = run(&server, &asked);
    assert_eq!(all.errors, Vec::<String>::new());
    assert!(matches!(all.sent[0].outcome, SendOutcome::Sent));
    assert!(matches!(&all.sent[1].outcome, SendOutcome::Held { lost: false, why } if why.contains("Bcc")));
    // A second run of the same send is known to the server by its send id: nothing goes twice.
    run(&server, &asked);
    let s = server.state.borrow();
    assert_eq!(s.sent.len(), 1);
    let text = String::from_utf8_lossy(&s.sent[0]).to_string();
    assert!(text.contains("Subject: Hello from MailCraft") && !text.contains("hidden@example.org"));
    assert!(s.items.values().any(|m| m.folder == 5 && m.subject == "Hello from MailCraft"));
}
