// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a directory run makes of an upload and of a vCard.

use std::future::{Future, ready};
use std::sync::Mutex;

use mailcraft_model::remote::{ContactChangeKind, ContactOp, ContactsUpload, EventChangeKind, EventFields, EventOp, EventTime};
use mailcraft_model::remote_tasks::{TaskOp, TaskOpKind};
use mailcraft_model::{Importance, TaskFields, TaskPatch};

use super::*;

/// An exchange that records the one request and answers a `DeleteItem` with success.
struct Recording {
    body: Mutex<String>,
}

impl crate::http::HttpExchange for Recording {
    fn exchange(&self, request: &crate::http::HttpRequest) -> impl Future<Output = Result<crate::http::HttpResponse, crate::http::TransportError>> {
        *self.body.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = String::from_utf8_lossy(request.body()).into_owned();
        let body = crate::standin::response(
            "DeleteItem",
            "<m:DeleteItemResponseMessage ResponseClass=\"Success\"><m:ResponseCode>NoError</m:ResponseCode></m:DeleteItemResponseMessage>",
        );
        ready(Ok(crate::http::HttpResponse::new(200, body.into_bytes())))
    }
}

#[test]
fn a_contact_deletion_goes_as_a_delete_item_and_needs_no_card() {
    let recording = Recording { body: Mutex::new(String::new()) };
    let client = crate::Client::new("http://127.0.0.1/EWS/Exchange.asmx", crate::Secret::new("test")).unwrap();
    let request = mailcraft_model::sync::EwsDirectoryRequest {
        account: 1,
        email: "ada@example.org".into(),
        url: "http://127.0.0.1/EWS/Exchange.asmx".into(),
        contact_changes: Some(ContactsUpload {
            account: 1,
            email: "ada@example.org".into(),
            ops: vec![ContactOp { kind: ContactChangeKind::Delete, contact: 7, remote: "contact-id".into(), ..Default::default() }],
        }),
        ..Default::default()
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let result = crate::standin::block_on(sync_directory(&client, &recording, &request, &cancel, &mut |_| true)).unwrap();
    assert_eq!(result.contacts_uploaded.as_ref().unwrap().outcomes[0].result, mailcraft_model::remote::EventResult::Done);
    let body = recording.body.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    assert!(body.contains("<m:DeleteItem") && body.contains("contact-id"), "{body}");
}

#[test]
fn an_event_to_make_has_the_fields_it_was_sent_with() {
    let op = EventOp {
        kind: EventChangeKind::Create,
        fields: EventFields {
            title: Some("Lunch".into()),
            time: Some(EventTime { start: 1_800, end: 3_600, all_day: false }),
            importance: Some(Importance::High),
            ..Default::default()
        },
        ..Default::default()
    };
    let event = event_from_fields(&op, "");
    assert_eq!(event.title, "Lunch");
    assert_eq!((event.start, event.end), (1_800, 3_600));
    assert_eq!(event.importance, Importance::High);
    assert!(!event.all_day);
}

#[test]
fn a_contact_from_a_vcard_keeps_its_people_and_places() {
    let card = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Grace Hopper\r\nN:Hopper;Grace;;;\r\nEMAIL;TYPE=WORK:grace@example.org\r\nTEL;TYPE=CELL:+1 555 0100\r\nEND:VCARD\r\n";
    let contact = contact_from_card(card).unwrap();
    assert_eq!(contact.display_name, "Grace Hopper");
    assert_eq!(contact.last, "Hopper");
    assert_eq!(contact.emails[0].value, "grace@example.org");
    assert_eq!(contact.phones[0].value, "+1 555 0100");
}

#[test]
fn a_stand_in_answers_a_directory_read() {
    let standin = crate::standin::StandIn::new();
    let client = crate::Client::new("http://127.0.0.1/EWS/Exchange.asmx", crate::Secret::new(crate::standin::AUTHORIZATION)).unwrap();
    let request = mailcraft_model::sync::EwsDirectoryRequest {
        account: 1,
        email: "ada@example.org".into(),
        url: "http://127.0.0.1/EWS/Exchange.asmx".into(),
        calendars: true,
        contacts: true,
        tasks: true,
        from: 1,
        to: 2_000_000_000,
        ..Default::default()
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let result = crate::standin::block_on(sync_directory(&client, &standin, &request, &cancel, &mut |_| true)).unwrap();
    assert!(result.errors.is_empty(), "{}", result.errors.join("; "));
    assert!(!result.auth_refused);
}

#[test]
fn a_task_patch_becomes_a_to_do() {
    let op = TaskOp {
        kind: TaskOpKind::Create,
        patch: TaskPatch { title: Some("Send it".into()), importance: Some(Importance::Low), ..Default::default() },
        ..Default::default()
    };
    let task = task_from_patch(&op);
    assert_eq!(task.title, "Send it");
    assert_eq!(task.importance, Importance::Low);
    let _ = TaskFields::default();
}
