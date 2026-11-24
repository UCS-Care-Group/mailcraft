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

struct RefusedUpdate;

impl crate::http::HttpExchange for RefusedUpdate {
    fn exchange(&self, request: &crate::http::HttpRequest) -> impl Future<Output = Result<crate::http::HttpResponse, crate::http::TransportError>> {
        assert!(String::from_utf8_lossy(request.body()).contains("<m:UpdateItem"));
        let body = crate::standin::response(
            "UpdateItem",
            r#"<m:UpdateItemResponseMessage ResponseClass="Error"><m:ResponseCode>ErrorAccessDenied</m:ResponseCode></m:UpdateItemResponseMessage>"#,
        );
        ready(Ok(crate::http::HttpResponse::new(200, body.into_bytes())))
    }
}

#[test]
fn an_event_update_refused_inside_http_success_is_denied() {
    let client = crate::Client::new("http://127.0.0.1/EWS/Exchange.asmx", crate::Secret::new("test")).unwrap();
    let request = mailcraft_model::sync::EwsDirectoryRequest {
        changes: Some(mailcraft_model::remote::EventsUpload {
            ops: vec![EventOp {
                kind: EventChangeKind::Update,
                remote: "event-id".into(),
                fields: EventFields { title: Some("New title".into()), ..Default::default() },
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let result = crate::standin::block_on(sync_directory(&client, &RefusedUpdate, &request, &cancel, &mut |_| true)).unwrap();
    assert_eq!(result.uploaded.unwrap().outcomes[0].result, EventResult::Denied);
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

/// The stand-in, with every request it was asked written down.
struct Logged {
    standin: crate::standin::StandIn,
    bodies: Mutex<Vec<String>>,
}

impl crate::http::HttpExchange for Logged {
    fn exchange(&self, request: &crate::http::HttpRequest) -> impl Future<Output = Result<crate::http::HttpResponse, crate::http::TransportError>> {
        self.bodies.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(String::from_utf8_lossy(request.body()).into_owned());
        self.standin.exchange(request)
    }
}

/// A read of the calendar alone (#1274) asks for the calendars and nothing else: no address book,
/// no to-do list.
#[test]
fn a_calendar_read_asks_for_the_calendars_alone() {
    let logged = Logged { standin: crate::standin::StandIn::new(), bodies: Mutex::new(Vec::new()) };
    let client = crate::Client::new("http://127.0.0.1/EWS/Exchange.asmx", crate::Secret::new(crate::standin::AUTHORIZATION)).unwrap();
    let request = mailcraft_model::sync::EwsDirectoryRequest {
        account: 1,
        email: "ada@example.org".into(),
        url: "http://127.0.0.1/EWS/Exchange.asmx".into(),
        calendars: true,
        from: 1_791_000_000,
        to: 1_793_000_000,
        background: true,
        ..Default::default()
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let result = crate::standin::block_on(sync_directory(&client, &logged, &request, &cancel, &mut |_| true)).unwrap();
    assert!(result.errors.is_empty(), "{}", result.errors.join("; "));
    assert!(result.calendars.is_some() && result.contacts.is_none() && result.tasks.is_none());
    assert!(result.events.iter().all(|download| (download.from, download.to) == (request.from, request.to)));
    let bodies = logged.bodies.lock().unwrap().clone();
    assert!(bodies.iter().any(|body| body.contains("\"calendar\"")), "{bodies:?}");
    assert!(bodies.iter().all(|body| !body.contains("\"contacts\"") && !body.contains("\"tasks\"")), "{bodies:?}");
    assert!(bodies.iter().all(|body| !body.contains("CreateItem") && !body.contains("UpdateItem") && !body.contains("DeleteItem")));
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

struct TruncatedCalendar(crate::standin::StandIn);

impl crate::http::HttpExchange for TruncatedCalendar {
    async fn exchange(&self, request: &crate::http::HttpRequest) -> Result<crate::http::HttpResponse, crate::http::TransportError> {
        let body = String::from_utf8_lossy(request.body());
        if !body.contains("<m:CalendarView") {
            return self.0.exchange(request).await;
        }
        let first = body.contains("StartDate=\"2026-11-01T00:00:00Z\"");
        let second = body.contains("EndDate=\"2026-11-03T00:00:00Z\"");
        let last = !(first && second);
        let item = |id: &str, start: &str, end: &str| {
            format!(r#"<t:CalendarItem><t:ItemId Id="{id}" /><t:Start>{start}</t:Start><t:End>{end}</t:End></t:CalendarItem>"#)
        };
        let mut items = item("long", "2026-11-01T00:00:00Z", "2026-11-03T00:00:00Z");
        if second && last {
            items.push_str(&item("short", "2026-11-02T12:00:00Z", "2026-11-02T13:00:00Z"));
        }
        let reply = crate::standin::response(
            "FindItem",
            &format!(
                r#"<m:FindItemResponseMessage ResponseClass="Success"><m:ResponseCode>NoError</m:ResponseCode><m:RootFolder IncludesLastItemInRange="{last}"><t:Items>{items}</t:Items></m:RootFolder></m:FindItemResponseMessage>"#
            ),
        );
        Ok(crate::http::HttpResponse::new(200, reply.into_bytes()))
    }
}

#[test]
fn an_overlapping_long_event_does_not_hide_the_next_appointment() {
    let client = crate::Client::new("http://127.0.0.1/EWS/Exchange.asmx", crate::Secret::new(crate::standin::AUTHORIZATION)).unwrap();
    let server = TruncatedCalendar(crate::standin::StandIn::new());
    let request = mailcraft_model::sync::EwsDirectoryRequest {
        calendars: true,
        from: mailcraft_time::iso_to_timestamp("2026-11-01T00:00:00Z", 0).unwrap(),
        to: mailcraft_time::iso_to_timestamp("2026-11-03T00:00:00Z", 0).unwrap(),
        ..Default::default()
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let result = crate::standin::block_on(sync_directory(&client, &server, &request, &cancel, &mut |_| true)).unwrap();
    let main = result.events.iter().find(|d| d.calendar.is_empty()).unwrap();
    assert!(main.complete);
    assert_eq!(main.events.iter().filter(|e| e.id == "long").count(), 1);
    assert!(main.events.iter().any(|e| e.id == "short"));
}

/// Route the named mailbox to a stand-in of its own. Its single-mailbox requests
/// have the same folder IDs and shapes without the mailbox qualifier.
struct SharedDirectory(crate::standin::StandIn);

impl crate::http::HttpExchange for SharedDirectory {
    async fn exchange(&self, request: &crate::http::HttpRequest) -> Result<crate::http::HttpResponse, crate::http::TransportError> {
        let body = String::from_utf8_lossy(request.body()).replace("<t:Mailbox><t:EmailAddress>shared@example.org</t:EmailAddress></t:Mailbox>", "");
        let request = crate::http::HttpRequest::soap(request.url(), &crate::Secret::new(crate::standin::AUTHORIZATION), body).unwrap();
        self.0.exchange(&request).await
    }
}

#[test]
fn shared_folders_alone_cannot_replace_the_account_directory() {
    let client = crate::Client::new("http://127.0.0.1/EWS/Exchange.asmx", crate::Secret::new(crate::standin::AUTHORIZATION)).unwrap();
    let server = SharedDirectory(crate::standin::StandIn::new());
    let request = mailcraft_model::sync::EwsDirectoryRequest {
        shared: vec![mailcraft_model::sync::EwsSharedMailbox {
            address: "shared@example.org".into(),
            calendar: true,
            contacts: true,
            ..Default::default()
        }],
        ..Default::default()
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let result = crate::standin::block_on(sync_directory(&client, &server, &request, &cancel, &mut |_| true)).unwrap();
    assert!(!result.calendars.unwrap().complete);
    assert!(!result.contacts.unwrap().complete);
}
