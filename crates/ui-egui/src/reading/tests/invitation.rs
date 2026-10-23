// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Meetings that arrive as mail, in the reading pane and in a message's own window: what the bar
//! says of each kind, what its buttons run and ask, what it says once answered, and that nothing
//! a message can say makes it offer a button that fails, or brings anything down.
//!
//! The messages are invented here; nobody in them exists.

use std::sync::{Arc, Mutex};

use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use mailcraft_model::remote::RemoteEvent;
use mailcraft_model::{Address, Attachment, Attendee, Body, FolderKind, Mailbox, Message, Response};
use serde_json::{Value, json};

use super::{NOW, Window, choose, click_at, says, select, window};
use crate::calendar_sync::testing::{Server, event, microsoft_account, serve};
use crate::dialogs::Dialog;
use crate::{MailApp, Services};

/// The address of the account the meetings are sent to.
const OWNER: &str = "avery@mail.example";

/// A meeting as mail has it. The clock of the tests is UTC, so `…Z` times read as they are.
#[derive(Clone)]
struct Meeting {
    method: &'static str,
    uid: String,
    title: &'static str,
    start: &'static str,
    end: &'static str,
    organizer: &'static str,
    /// More lines of the event (`SEQUENCE:2`, `RRULE:…`).
    more: String,
}

impl Default for Meeting {
    fn default() -> Self {
        Meeting {
            method: "REQUEST",
            uid: "uid-design-review".into(),
            title: "Design review",
            // Thursday the 15th, two to three in the afternoon: tomorrow.
            start: "DTSTART:20261015T140000Z",
            end: "DTEND:20261015T150000Z",
            organizer: "ORGANIZER;CN=Bo Marsh:mailto:bo@example.org",
            more: String::new(),
        }
    }
}

impl Meeting {
    fn ics(&self) -> String {
        let Meeting { method, uid, title, start, end, organizer, more } = self;
        let mut lines = vec![
            "BEGIN:VCALENDAR".to_string(),
            "VERSION:2.0".into(),
            "PRODID:-//Example//Meetings 1.0//EN".into(),
            format!("METHOD:{method}"),
            "BEGIN:VEVENT".into(),
            format!("UID:{uid}"),
            format!("SUMMARY:{title}"),
            "LOCATION:Room 4".into(),
        ];
        lines.extend([*start, *end, *organizer].iter().filter(|line| !line.is_empty()).map(|line| line.to_string()));
        if *method != "REPLY" {
            lines.push(format!("ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE;CN=Avery Quinn:mailto:{OWNER}"));
            lines.push("ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;CN=Cy Tran:mailto:cy@example.org".into());
            lines.push("ATTENDEE;ROLE=OPT-PARTICIPANT;CN=Maya Okafor:mailto:maya@example.org".into());
        }
        lines.extend(more.lines().map(str::to_string));
        lines.extend(["END:VEVENT".to_string(), "END:VCALENDAR".into()]);
        lines.join("\r\n") + "\r\n"
    }

    /// The message that carries it, from Bo.
    fn mail(&self) -> Message {
        carrying(self.title, self.ics().into_bytes())
    }
}

/// A message from Bo with a calendar part of these bytes.
fn carrying(subject: &str, calendar: Vec<u8>) -> Message {
    let text = "Hello Avery,\n\nCan you make it? The agenda is attached to the wiki page.\n\nBo".to_string();
    Message {
        message_id: format!("{}.{}@example.org", subject.len(), calendar.len()),
        from: Address::new("Bo Marsh", "bo@example.org"),
        to: vec![Address::new("Avery Quinn", OWNER)],
        subject: subject.to_string(),
        date: NOW - 3_600,
        received: NOW - 3_600,
        read: true,
        body: Body { text, html: None },
        attachments: vec![Attachment::new("invite.ics", "text/calendar", calendar)],
        ..Message::default()
    }
}

/// An app whose account is on a mail server (and keeps its calendar on this computer), with
/// `messages` in its Inbox. The account, and the messages' ids.
fn mail_app(messages: Vec<Message>) -> (MailApp, u64, Vec<u64>) {
    let mut app = MailApp::new(Session::at(Mailbox::default(), NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    let account = app.execute("account.add", json!({"email": OWNER, "name": "Avery Quinn", "incoming": {"host": "imap.mail.example"}})).unwrap();
    let account = account["id"].as_u64().unwrap();
    let ids = deliver(&mut app, account, messages);
    (app, account, ids)
}

/// Put `messages` into the Inbox of `account`.
fn deliver(app: &mut MailApp, account: u64, messages: Vec<Message>) -> Vec<u64> {
    let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let ids = messages.into_iter().map(|m| app.session.mailbox.add_message(Message { folder: inbox, ..m }).unwrap()).collect();
    app.session.mailbox_changed();
    app.session.view.folder = inbox;
    ids
}

fn hit(harness: &Window, kind: &str, id: u64) -> Option<egui::Rect> {
    harness.state().hit.rect(kind, id)
}

/// Click a part of the bar of message `id`.
fn press(harness: &mut Window, kind: &str, id: u64) {
    let at = hit(harness, kind, id).unwrap_or_else(|| panic!("the bar has no {kind}"));
    click_at(harness, at.center());
}

/// The question that is being asked: its title, and its ways of going ahead.
fn asked(harness: &Window) -> Option<(String, Vec<String>)> {
    match &harness.state().dialog {
        Some(Dialog::Confirm { ask, .. }) => {
            Some((ask.title.clone(), std::iter::once(ask.action.clone()).chain(ask.more.iter().map(|(label, _)| label.clone())).collect()))
        }
        _ => None,
    }
}

/// Answer the question that is being asked by clicking its button `label`.
fn answer(harness: &mut Window, label: &str) {
    let at = harness.get_by_label(label).rect().center();
    click_at(harness, at);
    harness.run_steps(3);
}

/// What `calendar.invitation` says of a message.
fn said(harness: &mut Window, id: u64) -> Value {
    harness.state_mut().execute("calendar.invitation", json!({"message": id})).unwrap()
}

/// What waits in the account's Outbox: who each message is to, its subject, and its calendar part.
fn outbox(app: &MailApp, account: u64) -> Vec<(String, String, String)> {
    let folder = app.session.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    let part = |m: &Message| m.attachments.first().map(|a| String::from_utf8_lossy(&a.data).into_owned()).unwrap_or_default();
    let mut waiting: Vec<_> = app
        .session
        .mailbox
        .messages_in(folder)
        .map(|m| (m.id, m.to.iter().map(|a| a.email.clone()).collect::<Vec<_>>().join(","), m.subject.clone(), part(m)))
        .collect();
    waiting.sort();
    waiting.into_iter().map(|(_, to, subject, part)| (to, subject, part)).collect()
}

/// The events of the calendar that are called `title`.
fn events(app: &MailApp, title: &str) -> Vec<mailcraft_model::Event> {
    app.session.mailbox.events.values().filter(|e| e.title == title).map(|e| e.as_ref().clone()).collect()
}

const ANSWERS: [&str; 3] = ["invitationAccept", "invitationTentative", "invitationDecline"];

#[test]
fn a_meeting_request_says_what_it_is_and_offers_the_answers() {
    for theme in ["light", "dark"] {
        let (mut app, _, ids) =
            mail_app(vec![Meeting::default().mail(), super::message(1, ("Jo", "jo@example.org"), "Lunch?", None, "Lunch?".into())]);
        app.execute("ui.theme", json!({"value": theme})).unwrap();
        let (request, plain) = (ids[0], ids[1]);
        let mut harness = window(app, (1280.0, 900.0));
        select(&mut harness, request);
        // The ways to answer, each with its menu, and the way to its day in the calendar.
        for kind in ANSWERS.iter().chain(&["invitationAcceptMenu", "invitationTentativeMenu", "invitationDeclineMenu", "invitationCalendar"]) {
            assert!(hit(&harness, kind, request).is_some(), "{theme}: no {kind}");
        }
        assert!(hit(&harness, "invitationRemove", request).is_none() && hit(&harness, "invitationFind", request).is_none());
        // What it is: when (with the weekday), where, who.
        assert!(says(&harness, "You haven't answered this invitation yet."));
        assert!(says(&harness, "When Thursday, October 15, 2026 2:00 PM – 3:00 PM"), "{theme}");
        assert!(says(&harness, "Where Room 4") && says(&harness, "Organizer Bo Marsh"));
        assert!(says(&harness, "Required Avery Quinn; Cy Tran") && says(&harness, "Optional Maya Okafor"));
        assert!(says(&harness, "Nothing else is on your calendar at that time."));
        // The buttons are where the bar is: between the header and the body.
        let accept = hit(&harness, "invitationAccept", request).unwrap();
        let body = super::super::view::kept(&harness.ctx, request, |_, origin| origin).unwrap();
        assert!(accept.max.y < body.y && accept.min.y > 100.0, "{accept:?} {body:?}");
        // Something at the same time, and something right after: said under it.
        harness.state_mut().execute("calendar.new", json!({"title": "Dentist", "start": "2026-10-15T14:30", "end": "2026-10-15T15:00"})).unwrap();
        harness.state_mut().execute("calendar.new", json!({"title": "Stand-up", "start": "2026-10-15T15:00", "end": "2026-10-15T15:15"})).unwrap();
        harness.state_mut().execute("calendar.new", json!({"title": "Free hour", "start": "2026-10-15T14:00", "showAs": "free"})).unwrap();
        harness.run_steps(3);
        assert!(says(&harness, "At the same time: Dentist (2:30 PM – 3:00 PM)"), "{theme}");
        assert!(says(&harness, "Right before or after: Stand-up (3:00 PM – 3:15 PM)"));
        assert!(!says(&harness, "Nothing else is on your calendar") && !says(&harness, "Free hour (2:00"));
        // A message that carries no meeting has no bar.
        select(&mut harness, plain);
        assert!(ANSWERS.iter().all(|kind| hit(&harness, kind, plain).is_none()) && !says(&harness, "Organizer"));
    }
}

#[test]
fn a_request_as_it_comes_off_the_wire_is_a_meeting_request() {
    // As a big mail server sends one: the calendar part is a third form of the body, in base64,
    // with the times in a zone of its own.
    let ics = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nPRODID:Example Server\r\nVERSION:2.0\r\nBEGIN:VTIMEZONE\r\nTZID:Example Standard Time\r\n\
BEGIN:STANDARD\r\nDTSTART:16010101T020000\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0000\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\n\
ORGANIZER;CN=Bo Marsh:mailto:bo@example.org\r\nATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE;CN=Avery Quinn:mailto:avery@mail.example\r\n\
UID:040000008200E00074C5B7101A82E00800000000\r\nSUMMARY;LANGUAGE=en-US:Design review\r\nDTSTART;TZID=Example Standard Time:20261015T140000\r\n\
DTEND;TZID=Example Standard Time:20261015T150000\r\nSEQUENCE:0\r\nLOCATION;LANGUAGE=en-US:Room 4\r\nX-MICROSOFT-CDO-BUSYSTATUS:TENTATIVE\r\n\
BEGIN:VALARM\r\nTRIGGER:-PT15M\r\nACTION:DISPLAY\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let encoded = mailcraft_engine::cmd::base64_encode(ics.as_bytes());
    let folded: Vec<String> = encoded.as_bytes().chunks(76).map(|line| String::from_utf8_lossy(line).into_owned()).collect();
    let eml = [
        "From: Bo Marsh <bo@example.org>",
        "To: Avery Quinn <avery@mail.example>",
        "Subject: Design review",
        "Date: Wed, 14 Oct 2026 15:00:00 +0000",
        "Message-ID: <request-1@example.org>",
        "MIME-Version: 1.0",
        "Content-Type: multipart/alternative; boundary=\"_000_part_\"",
        "",
        "--_000_part_",
        "Content-Type: text/plain; charset=\"utf-8\"",
        "",
        "Can you make it?",
        "--_000_part_",
        "Content-Type: text/html; charset=\"utf-8\"",
        "",
        "<html><body><p>Can you make it?</p></body></html>",
        "--_000_part_",
        "Content-Type: text/calendar; charset=\"utf-8\"; method=REQUEST",
        "Content-Transfer-Encoding: base64",
        "",
        &folded.join("\r\n"),
        "--_000_part_--",
        "",
    ]
    .join("\r\n");
    let (mut app, account, _) = mail_app(Vec::new());
    let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let data = mailcraft_engine::cmd::base64_encode(eml.as_bytes());
    app.execute("file.import", json!({"name": "request.eml", "data": data, "folder": inbox})).unwrap();
    app.session.view.folder = inbox;
    let request = app.session.mailbox.messages.values().find(|m| m.subject == "Design review").map(|m| m.id).unwrap();
    assert_eq!(super::super::invitation::list_marks(app.session.mailbox.message(request).unwrap()), (Some("meetingRequest"), false));
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, request);
    assert!(ANSWERS.iter().all(|kind| hit(&harness, kind, request).is_some()));
    assert!(
        says(&harness, "When Thursday, October 15, 2026 2:00 PM – 3:00 PM") && says(&harness, "Where Room 4") && says(&harness, "Organizer Bo Marsh")
    );
    // Its text is the message's text: the calendar part is not shown as a file, and nothing of it leaks into the body.
    let body =
        super::super::view::kept(&harness.ctx, request, |laid, _| laid.items.iter().map(super::text_of).collect::<Vec<_>>().join("\n")).unwrap();
    assert!(body.contains("Can you make it?") && !body.contains("VCALENDAR") && !says(&harness, "VCALENDAR"), "{body}");
    // Accepted without a word to anybody: on the calendar, with the reminder it asked for.
    press(&mut harness, "invitationAccept", request);
    answer(&mut harness, "Don't Send a Response");
    let here = events(harness.state(), "Design review");
    assert_eq!(here.len(), 1);
    assert_eq!(
        (here[0].uid.as_str(), here[0].reminder_minutes, here[0].location.as_str()),
        ("040000008200E00074C5B7101A82E00800000000", Some(15), "Room 4")
    );
    assert!(outbox(harness.state(), account).is_empty());
}

/// (A8-5) The answer is mailed to the organizer's address, which the bar and the question show,
/// with a word when the message came from another address.
#[test]
fn the_address_an_answer_goes_to_is_shown() {
    let forged = Meeting { uid: "uid-other".into(), organizer: "ORGANIZER;CN=IT Helpdesk:mailto:collector@evil.example", ..Meeting::default() };
    let (app, _, ids) = mail_app(vec![Meeting::default().mail(), forged.mail()]);
    let mut harness = window(app, (1280.0, 900.0));
    let question = |harness: &Window| match &harness.state().dialog {
        Some(Dialog::Confirm { ask, .. }) => ask.text.clone(),
        _ => panic!("no question"),
    };
    select(&mut harness, ids[0]);
    assert!(says(&harness, "Organizer Bo Marsh <bo@example.org>") && !says(&harness, "didn't come from the address"));
    press(&mut harness, "invitationAccept", ids[0]);
    let text = question(&harness);
    assert!(text.contains("Bo Marsh <bo@example.org> organizes") && !text.contains("didn't come from"), "{text}");
    answer(&mut harness, "Don't Send a Response");
    select(&mut harness, ids[1]);
    assert!(says(&harness, "Organizer IT Helpdesk <collector@evil.example>") && says(&harness, "didn't come from the address"));
    press(&mut harness, "invitationAccept", ids[1]);
    let text = question(&harness);
    assert!(text.contains("IT Helpdesk <collector@evil.example> organizes") && text.contains("didn't come from the address"), "{text}");
}

#[test]
fn a_click_on_an_answer_asks_before_anything_is_sent() {
    let (app, account, ids) = mail_app(vec![Meeting::default().mail()]);
    let request = ids[0];
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, request);
    // Accept: the question, and until it is answered nothing is in the calendar or the Outbox.
    press(&mut harness, "invitationAccept", request);
    let (title, ways) = asked(&harness).expect("no question was asked");
    assert_eq!((title.as_str(), ways), ("Send a Response?", vec!["Send the Response Now".to_string(), "Don't Send a Response".to_string()]));
    assert!(events(harness.state(), "Design review").is_empty() && outbox(harness.state(), account).is_empty());
    assert!(!harness.state().session.mailbox.message(request).unwrap().answered);
    // Not sent: the meeting is in the calendar, accepted, and nobody is written to.
    answer(&mut harness, "Don't Send a Response");
    assert!(harness.state().dialog.is_none());
    let here = events(harness.state(), "Design review");
    assert_eq!(here.len(), 1);
    let mine = here[0].attendees.iter().find(|who| who.address.email == OWNER).map(|who| who.response);
    assert_eq!((mine, here[0].start.iso().as_str()), (Some(Response::Accepted), "2026-10-15T14:00:00"));
    assert!(outbox(harness.state(), account).is_empty());
    // The bar says what was answered and when, and still offers the answers.
    harness.run_steps(3);
    assert!(says(&harness, "Accepted on Wed 10/14/2026 4:00 PM"));
    assert!(!says(&harness, "You haven't answered") && !says(&harness, "You replied to this message."));
    assert!(ANSWERS.iter().all(|kind| hit(&harness, kind, request).is_some()));
    // Changed to Decline, and sent with a note: out of the calendar, and a REPLY for Bo waits.
    press(&mut harness, "invitationDecline", request);
    assert!(asked(&harness).is_some());
    let Some(Dialog::Confirm { ask, .. }) = &mut harness.state_mut().dialog else { panic!() };
    ask.note.as_mut().unwrap().1 = "Out that day".into();
    harness.run_steps(2);
    answer(&mut harness, "Send the Response Now");
    assert!(events(harness.state(), "Design review").is_empty());
    let waiting = outbox(harness.state(), account);
    assert_eq!(waiting.len(), 1);
    assert_eq!((waiting[0].0.as_str(), waiting[0].1.as_str()), ("bo@example.org", "Declined: Design review"));
    assert!(waiting[0].2.contains("METHOD:REPLY\r\n") && waiting[0].2.contains("PARTSTAT=DECLINED"), "{}", waiting[0].2);
    let note =
        harness.state().session.mailbox.messages.values().find(|m| m.subject == "Declined: Design review").map(|m| m.body.text.clone()).unwrap();
    assert!(note.starts_with("Out that day\n"), "{note}");
    harness.run_steps(3);
    assert!(says(&harness, "Declined on Wed 10/14/2026 4:00 PM"));
    // Cancel: as if the button had never been pressed.
    press(&mut harness, "invitationTentative", request);
    assert!(asked(&harness).is_some());
    answer(&mut harness, "Cancel");
    assert!(harness.state().dialog.is_none() && events(harness.state(), "Design review").is_empty());
    assert_eq!(outbox(harness.state(), account).len(), 1);
}

#[test]
fn the_menu_beside_an_answer_offers_its_three_ways() {
    let (app, account, ids) = mail_app(vec![Meeting::default().mail()]);
    let request = ids[0];
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, request);
    let open = |harness: &mut Window, kind: &str| {
        press(harness, kind, request);
        harness.run_steps(2);
        hit(harness, kind, request).unwrap().center()
    };
    // Sent at once, without a question.
    let at = open(&mut harness, "invitationTentativeMenu");
    for label in ["Write a Note with the Response…", "Send the Response Now", "Don't Send a Response"] {
        assert!(harness.query_by_label(label).is_some(), "the menu has no {label}");
    }
    choose(&mut harness, "Send the Response Now", at);
    harness.run_steps(4);
    assert!(harness.state().dialog.is_none());
    assert_eq!(outbox(harness.state(), account).iter().map(|m| m.1.as_str()).collect::<Vec<_>>(), ["Tentative: Design review"]);
    assert_eq!(events(harness.state(), "Design review").len(), 1);
    assert!(says(&harness, "Tentatively accepted on Wed 10/14/2026 4:00 PM"));
    // Not sent, without a question.
    let at = open(&mut harness, "invitationAcceptMenu");
    choose(&mut harness, "Don't Send a Response", at);
    harness.run_steps(4);
    assert!(harness.state().dialog.is_none() && outbox(harness.state(), account).len() == 1);
    assert!(says(&harness, "Accepted on"));
    // With a note first: the question, which has the field for it.
    let at = open(&mut harness, "invitationDeclineMenu");
    choose(&mut harness, "Write a Note with the Response…", at);
    harness.run_steps(4);
    let Some(Dialog::Confirm { ask, .. }) = &harness.state().dialog else { panic!("no question") };
    assert!(ask.note.is_some());
}

#[test]
fn an_account_that_cannot_send_answers_in_its_calendar_only() {
    // A mailbox on this computer only: no server to send an answer through.
    let mut mailbox = Mailbox::with_local_account("Avery Quinn", OWNER);
    let inbox = mailbox.inbox().unwrap();
    let request = mailbox.add_message(Message { folder: inbox, ..Meeting::default().mail() }).unwrap();
    let mut app = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    (app.autosave, app.auto_sync) = (false, false);
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, request);
    assert_eq!(said(&mut harness, request)["canSend"], false);
    // No question about sending what can't be sent: the meeting goes into the calendar.
    press(&mut harness, "invitationAccept", request);
    harness.run_steps(3);
    assert!(harness.state().dialog.is_none());
    assert_eq!(events(harness.state(), "Design review").len(), 1);
    assert!(says(&harness, "Accepted on"));
}

/// The app with a Microsoft account whose calendar the stand-in keeps, and what it has taken in.
fn microsoft(events: Vec<RemoteEvent>) -> (MailApp, Arc<Mutex<Server>>, u64) {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let server = serve(&mut app);
    let account = microsoft_account(&mut app);
    server.lock().unwrap().events = events;
    sync(&mut app);
    (app, server, account)
}

/// Let what waits go to the stand-in, bring the days on screen down, and take its answer in.
fn sync(app: &mut MailApp) {
    static LATER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    // Each look is well after the last: nothing is held back for having just been asked.
    let later = LATER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as f64 * 3_600_000.0;
    for _ in 0..4 {
        app.poll_calendar();
    }
    // (Also for an app that starts nothing by itself: this look is the test's own.)
    let by_itself = std::mem::replace(&mut app.auto_sync, true);
    app.calendar_tick_at(true, crate::now_ms() + later);
    app.auto_sync = by_itself;
    for _ in 0..4 {
        app.poll_calendar();
    }
}

/// Bo's meeting as the stand-in for Microsoft 365 has it: the owner is invited.
fn invited(id: &str, uid: &str, start: i64) -> RemoteEvent {
    let who = |name: &str, email: &str| Attendee { address: Address::new(name, email), ..Default::default() };
    RemoteEvent {
        uid: uid.into(),
        location: "Room 4".into(),
        organizer: Some(Address::new("Bo Marsh", "bo@example.org")),
        attendees: vec![who("Avery Quinn", OWNER), who("Cy Tran", "cy@example.org")],
        is_organizer: Some(false),
        ..event(id, "Design review", start)
    }
}

/// Thursday 2026-10-15 14:00 UTC.
const THURSDAY: i64 = NOW + 22 * 3_600;

#[test]
fn on_a_server_the_request_and_the_calendars_item_are_one_meeting() {
    // The server has put the meeting on the calendar (the same UID, in other letters).
    let (mut app, server, account) = microsoft(vec![invited("g-1", "UID-DESIGN-REVIEW", THURSDAY)]);
    app.auto_sync = false;
    let request = deliver(&mut app, account, vec![Meeting::default().mail()])[0];
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, request);
    let meeting = said(&mut harness, request);
    assert_eq!((meeting["onServer"].as_bool(), meeting["answerable"].as_bool(), meeting["canSend"].as_bool()), (Some(true), Some(true), Some(true)));
    assert!(ANSWERS.iter().all(|kind| hit(&harness, kind, request).is_some()));
    // Accepted from the message, and sent with a note: the server is told to accept its item,
    // and it is the server that writes to the organizer. No mail of our own is made.
    press(&mut harness, "invitationAccept", request);
    assert_eq!(asked(&harness).map(|(title, _)| title).as_deref(), Some("Send a Response?"));
    assert!(server.lock().unwrap().mail.is_empty());
    let Some(Dialog::Confirm { ask, .. }) = &mut harness.state_mut().dialog else { panic!() };
    ask.note.as_mut().unwrap().1 = "See you there".into();
    harness.run_steps(2);
    answer(&mut harness, "Send the Response Now");
    sync(harness.state_mut());
    assert_eq!(server.lock().unwrap().mail, ["response:Design review:Accepted:See you there"]);
    assert!(outbox(harness.state(), account).is_empty());
    assert_eq!(server.lock().unwrap().events[0].response, Response::Accepted);
    harness.run_steps(3);
    assert!(says(&harness, "Accepted on Wed 10/14/2026 4:00 PM"));
    // Declined without a word: the item goes, and nobody is written to.
    press(&mut harness, "invitationDecline", request);
    answer(&mut harness, "Don't Send a Response");
    sync(harness.state_mut());
    assert_eq!(server.lock().unwrap().mail.len(), 1);
    assert!(server.lock().unwrap().events.is_empty() && events(harness.state(), "Design review").is_empty());
    harness.run_steps(3);
    assert!(says(&harness, "Declined on"));
}

#[test]
fn a_meeting_the_server_has_not_brought_yet_is_asked_for_and_offers_nothing_meanwhile() {
    // In December: far from the days the calendar keeps current by itself.
    let december = Meeting { uid: "uid-december".into(), start: "DTSTART:20261210T140000Z", end: "DTEND:20261210T150000Z", ..Default::default() };
    let (mut app, server, account) = microsoft(Vec::new());
    let request = deliver(&mut app, account, vec![december.mail()])[0];
    let mut harness = window(app, (1280.0, 900.0));
    let before = server.lock().unwrap().jobs.len();
    select(&mut harness, request);
    harness.run_steps(8);
    // Nothing to answer through: no button that could only fail, and the bar says why.
    assert_eq!(said(&mut harness, request)["blocked"], "calendar");
    assert!(ANSWERS.iter().all(|kind| hit(&harness, kind, request).is_none()));
    assert!(says(&harness, "This meeting isn't on your calendar here yet."));
    assert!(harness.state_mut().execute("calendar.respond", json!({"message": request, "response": "accept", "send": true})).is_err());
    // The server was asked for the days round the meeting, once, and nothing was sent up.
    let jobs: Vec<_> = server.lock().unwrap().jobs.iter().skip(before).cloned().collect();
    let round: Vec<_> = jobs.iter().filter(|job| job.calendar == Some((1_796_774_400, 1_797_033_600))).collect();
    assert_eq!(round.len(), 1, "{jobs:?}");
    assert!(round[0].ops.is_empty() && !round[0].directory);
    // It is there now. The owner asks: it comes down, and the answers are offered.
    let thursday = 1_796_911_200;
    server.lock().unwrap().events.push(invited("g-2", "uid-december", thursday));
    press(&mut harness, "invitationFind", request);
    harness.run_steps(8);
    assert!(events(harness.state(), "Design review").iter().any(|e| e.uid == "uid-december"));
    assert!(ANSWERS.iter().all(|kind| hit(&harness, kind, request).is_some()));
    assert!(!says(&harness, "isn't on your calendar here yet"));
    assert!(server.lock().unwrap().mail.is_empty());
}

#[test]
fn asking_the_calendar_about_a_meeting_sends_nothing_and_waits_its_turn() {
    use std::sync::mpsc;

    use crate::{GraphRequest, GraphResult};
    // A host that notes what it is asked, and answers when the test lets it.
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    (app.autosave, app.auto_sync) = (false, false);
    type Asked = (usize, bool, bool, Option<(i64, i64)>, bool);
    let asked: Arc<Mutex<Vec<Asked>>> = Default::default();
    let held: Arc<Mutex<Vec<mpsc::Sender<GraphResult>>>> = Default::default();
    let (noted, kept) = (asked.clone(), held.clone());
    app.services.graph = Some(Box::new(move |request: GraphRequest| {
        let (tx, rx) = mpsc::channel();
        noted.lock().unwrap().push((request.outbox.len(), request.changes.is_some(), request.directory, request.calendar, request.settings));
        kept.lock().unwrap().push(tx);
        rx
    }));
    let account = microsoft_account(&mut app);
    // Mail waits in the Outbox: none of it goes along.
    let outbox = app.session.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    app.session
        .mailbox
        .add_message(Message { folder: outbox, ..super::message(1, ("Avery Quinn", OWNER), "Waiting", None, "Waiting".into()) })
        .unwrap();
    let day = mailcraft_time::Date::new(2026, 12, 10).unwrap();
    // Asked: the three days round the meeting, and nothing else. No mail, no changes, not the
    // wide stretch, no settings.
    assert_eq!(app.calendar_job_around(account, day), Ok(()));
    assert_eq!(*asked.lock().unwrap(), [(0, false, false, Some((1_796_774_400, 1_797_033_600)), false)]);
    // One job at a time: while it is at work, no second one starts.
    assert!(app.calendar_syncing() && app.calendar_job_around(account, day).is_err());
    assert_eq!(asked.lock().unwrap().len(), 1);
    // It answers (here: the host is gone), and is taken in like any other.
    held.lock().unwrap().clear();
    app.poll_calendar();
    assert!(!app.calendar_syncing());
    // Something made here waits to go to the server: that goes first, in a job of the
    // calendar's own. This one never carries it, and doesn't start.
    let download = mailcraft_model::remote::EventsDownload {
        from: NOW - 86_400,
        to: NOW + 86_400,
        complete: true,
        events: vec![event("e-1", "Budget review", NOW)],
        ..Default::default()
    };
    let owner = app.session.mailbox.account(account).cloned().unwrap();
    mailcraft_engine::cmd::remote::apply_events(&mut app.session, &download.for_account(&owner)).unwrap();
    let review = app.session.mailbox.events.values().find(|e| e.title == "Budget review").map(|e| e.id).unwrap();
    app.execute("calendar.move", json!({"id": review, "start": "2026-10-15T09:00"})).unwrap();
    assert!(app.calendar_changes(account).is_some() && app.calendar_job_around(account, day).is_err());
    app.session.mailbox.event_changes.clear();
    // Nor while a Send/Receive is about to ask, nor before the owner has allowed the calendar.
    app.sync_queue.push_back((account, true));
    assert!(app.calendar_job_around(account, day).is_err());
    app.sync_queue.clear();
    app.graph_consent_needed.insert(account);
    assert!(app.calendar_job_around(account, day).is_err());
    app.graph_consent_needed.remove(&account);
    assert_eq!(asked.lock().unwrap().len(), 1);
    // An account that signs in nowhere, a day at the end of time, a build without a host.
    assert!(app.calendar_job_around(1, day).is_err() && app.calendar_job_around(u64::MAX, day).is_err());
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert_eq!(app.calendar_job_around(account, mailcraft_time::Date::new(9999, 12, 31).unwrap()), Ok(()));
    held.lock().unwrap().clear();
    app.poll_calendar();
    app.services.graph = None;
    assert!(app.calendar_job_around(account, day).is_err());
    // Asking as it did left alone when the days on screen were last asked for.
    assert!(!app.calendar.checked.contains_key(&account));
}

#[test]
fn an_update_a_cancellation_and_an_answer_each_say_what_they_are() {
    let update = Meeting { more: "SEQUENCE:2".into(), ..Default::default() };
    let cancel = Meeting { method: "CANCEL", ..Default::default() };
    let reply = Meeting {
        method: "REPLY",
        uid: "uid-mine".into(),
        organizer: "ORGANIZER;CN=Avery Quinn:mailto:avery@mail.example",
        more: "ATTENDEE;PARTSTAT=TENTATIVE;CN=Maya Okafor:mailto:maya@example.org".into(),
        ..Default::default()
    };
    let (app, account, ids) = mail_app(vec![update.mail(), cancel.mail(), reply.mail()]);
    let (update, cancel, reply) = (ids[0], ids[1], ids[2]);
    let mut harness = window(app, (1280.0, 900.0));
    // An update: said, and answered like the invitation.
    select(&mut harness, update);
    assert!(says(&harness, "The organizer has changed this meeting."));
    assert!(ANSWERS.iter().all(|kind| hit(&harness, kind, update).is_some()));
    harness.state_mut().execute("calendar.respond", json!({"message": update, "response": "accept"})).unwrap();
    assert_eq!(events(harness.state(), "Design review").len(), 1);
    // A cancellation: nothing to answer, and a way to take the meeting out of the calendar.
    select(&mut harness, cancel);
    assert!(says(&harness, "This meeting was called off by its organizer."));
    assert!(ANSWERS.iter().all(|kind| hit(&harness, kind, cancel).is_none()));
    assert!(hit(&harness, "invitationCalendar", cancel).is_some());
    press(&mut harness, "invitationRemove", cancel);
    harness.run_steps(3);
    assert!(harness.state().dialog.is_none() && events(harness.state(), "Design review").is_empty());
    assert!(says(&harness, "It is not on your calendar.") && hit(&harness, "invitationRemove", cancel).is_none());
    assert!(outbox(harness.state(), account).is_empty());
    // Somebody's answer: who said what, and nothing to press.
    select(&mut harness, reply);
    assert!(says(&harness, "Maya Okafor tentatively accepted this meeting."));
    for kind in ANSWERS.iter().chain(&["invitationCalendar", "invitationRemove", "invitationFind"]) {
        assert!(hit(&harness, kind, reply).is_none(), "{kind}");
    }
}

#[test]
fn a_meeting_that_is_over_or_the_owners_own_offers_no_answers() {
    let past = Meeting { uid: "uid-past".into(), start: "DTSTART:20261001T140000Z", end: "DTEND:20261001T150000Z", ..Default::default() };
    let own = Meeting { uid: "uid-own".into(), organizer: "ORGANIZER;CN=Avery Quinn:mailto:AVERY@mail.example", ..Default::default() };
    let series = Meeting { uid: "uid-series".into(), more: "RRULE:FREQ=WEEKLY;BYDAY=TH".into(), ..Default::default() };
    let ended = Meeting {
        uid: "uid-ended".into(),
        start: "DTSTART:20260903T140000Z",
        end: "DTEND:20260903T150000Z",
        more: "RRULE:FREQ=WEEKLY;COUNT=3".into(),
        ..Default::default()
    };
    let (app, _, ids) = mail_app(vec![past.mail(), own.mail(), series.mail(), ended.mail()]);
    let mut harness = window(app, (1280.0, 900.0));
    for (id, why, words) in
        [(ids[0], "past", "This meeting is over."), (ids[1], "organizer", "You organize this meeting"), (ids[3], "past", "This meeting is over.")]
    {
        select(&mut harness, id);
        assert_eq!(said(&mut harness, id)["blocked"], why);
        assert!(says(&harness, words), "{words}");
        assert!(ANSWERS.iter().all(|kind| hit(&harness, kind, id).is_none()), "{why}");
        assert!(harness.state_mut().run("calendar.respond", json!({"message": id, "response": "accept"})).is_ok());
    }
    // A series that goes on is said in words, and can be answered.
    select(&mut harness, ids[2]);
    assert!(says(&harness, "Every week on Thursday"));
    assert!(ANSWERS.iter().all(|kind| hit(&harness, kind, ids[2]).is_some()));
}

#[test]
fn the_calendar_button_shows_the_day_and_the_option_files_the_request_away() {
    let (app, account, ids) =
        mail_app(vec![Meeting::default().mail(), Meeting { uid: "uid-other".into(), title: "Budget", ..Default::default() }.mail()]);
    let (first, second) = (ids[0], ids[1]);
    let mut harness = window(app, (1280.0, 420.0));
    select(&mut harness, first);
    // Unless the owner turns it on, an answered request stays where it is.
    assert!(!harness.state().session.prefs.delete_answered_requests);
    let inbox = harness.state().session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let bin = harness.state().session.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    harness.state_mut().execute("calendar.respond", json!({"message": first, "response": "accept"})).unwrap();
    assert_eq!(harness.state().session.mailbox.message(first).map(|m| m.folder), Some(inbox));
    // Turned on in File › Options: the next one goes to Deleted Items, and is not deleted for good.
    harness.state_mut().execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    harness.run_steps(3);
    // A short window puts it below the fold, as a taller system font (Segoe UI on Windows) does.
    harness.get_by_label("Delete meeting requests from the Inbox after responding").scroll_to_me();
    harness.run_steps(3);
    harness.get_by_label("Delete meeting requests from the Inbox after responding").click();
    harness.run_steps(3);
    assert!(harness.state().session.prefs.delete_answered_requests && harness.state().prefs().options.delete_answered_requests);
    harness.state_mut().execute("ui.backstage", json!({"value": false})).unwrap();
    select(&mut harness, second);
    press(&mut harness, "invitationTentative", second);
    answer(&mut harness, "Don't Send a Response");
    assert_eq!(harness.state().session.mailbox.message(second).map(|m| m.folder), Some(bin));
    assert_eq!(events(harness.state(), "Budget").len(), 1);
    // Answered again where it lies: it stays there.
    harness.state_mut().execute("calendar.respond", json!({"message": second, "response": "accept"})).unwrap();
    assert_eq!(harness.state().session.mailbox.message(second).map(|m| m.folder), Some(bin));
    // "Calendar…": the calendar, on the day of the meeting, with the meeting selected.
    select(&mut harness, first);
    press(&mut harness, "invitationCalendar", first);
    harness.run_steps(3);
    let app = harness.state();
    assert_eq!((app.session.view.module, app.session.view.date.iso().as_str()), (mailcraft_engine::Module::Calendar, "2026-10-15"));
    assert_eq!(app.session.sel.event, events(app, "Design review").first().map(|e| e.id));
}

#[test]
fn a_message_in_a_window_of_its_own_has_the_bar_too() {
    let (app, _, ids) = mail_app(vec![Meeting::default().mail(), super::message(1, ("Jo", "jo@example.org"), "Lunch?", None, "Lunch?".into())]);
    let (request, plain) = (ids[0], ids[1]);
    let mut harness = window(app, (1280.0, 900.0));
    // The pane shows another message; the request is in a window.
    select(&mut harness, plain);
    harness.state_mut().execute("ui.openWindow", json!({"message": request})).unwrap();
    harness.run_steps(6);
    assert!(hit(&harness, "invitationAccept", request).is_none());
    assert!(says(&harness, "When Thursday, October 15, 2026 2:00 PM – 3:00 PM"));
    press(&mut harness, "windowInvitationAccept", request);
    assert_eq!(asked(&harness).map(|(title, _)| title).as_deref(), Some("Send a Response?"));
    answer(&mut harness, "Don't Send a Response");
    assert_eq!(events(harness.state(), "Design review").len(), 1);
    harness.run_steps(3);
    assert!(says(&harness, "Accepted on"));
}

#[test]
fn the_message_list_marks_meeting_mail() {
    let marks = |m: &Message| super::super::invitation::list_marks(m);
    let reply = |said: &str| {
        Meeting { method: "REPLY", more: format!("ATTENDEE;PARTSTAT={said};CN=Maya:mailto:maya@example.org"), ..Default::default() }.mail()
    };
    assert_eq!(marks(&Meeting::default().mail()), (Some("meetingRequest"), false));
    assert_eq!(marks(&Meeting { method: "CANCEL", ..Default::default() }.mail()), (Some("meetingCancelled"), false));
    assert_eq!(marks(&reply("ACCEPTED")).0, Some("meetingAccepted"));
    assert_eq!(marks(&reply("DECLINED")).0, Some("meetingDeclined"));
    assert_eq!(marks(&reply("TENTATIVE")).0, Some("meetingTentative"));
    assert_eq!(marks(&reply("DELEGATED")).0, Some("meetingRequest"));
    // A file besides the calendar part is still a file; a calendar file that is no meeting mail
    // is a file like any other, and so is junk.
    let mut with_file = Meeting::default().mail();
    with_file.attachments.push(Attachment::new("agenda.pdf", "application/pdf", vec![1, 2, 3]));
    assert_eq!(marks(&with_file), (Some("meetingRequest"), true));
    assert_eq!(marks(&Meeting { method: "PUBLISH", ..Default::default() }.mail()), (None, true));
    assert_eq!(marks(&carrying("Junk", vec![0xFF, 0x00, 0xFE, b'M', b'E'])), (None, true));
    assert_eq!(marks(&carrying("Empty", Vec::new())), (None, true));
    assert_eq!(marks(&super::message(1, ("Jo", "jo@example.org"), "Lunch?", None, "Lunch?".into())), (None, false));
    // And every one of them has an icon of its own.
    for name in ["meetingRequest", "meetingCancelled", "meetingAccepted", "meetingTentative", "meetingDeclined"] {
        assert!(crate::icons::NAMES.contains(&name), "{name}");
    }
    // In the window: the list paints them without a hitch.
    let (app, _, _) =
        mail_app(vec![Meeting::default().mail(), Meeting { method: "CANCEL", ..Default::default() }.mail(), reply("ACCEPTED"), with_file]);
    let mut harness = window(app, (1280.0, 900.0));
    harness.run_steps(4);
}

/// Invitations nobody should have sent.
fn hostile() -> Vec<(&'static str, Message)> {
    let odd = |what: Meeting| what.mail();
    let people: String = (0..5_000).map(|n| format!("ATTENDEE;CN=Person {n} {}:mailto:p{n}@example.org\r\n", "x".repeat(300))).collect();
    vec![
        ("no times", odd(Meeting { start: "", end: "", ..Default::default() })),
        ("times that are no times", odd(Meeting { start: "DTSTART:tomorrow", end: "DTEND:later", ..Default::default() })),
        ("the year 9999", odd(Meeting { start: "DTSTART:99991231T235959Z", end: "DTEND:99991231T235959Z", ..Default::default() })),
        ("the year 1", odd(Meeting { start: "DTSTART:00010101T000000Z", end: "DTEND:99991231T235959Z", ..Default::default() })),
        ("ends before it starts", odd(Meeting { start: "DTSTART:20261015T140000Z", end: "DTEND:19700101T000000Z", ..Default::default() })),
        ("whole days without end", odd(Meeting { start: "DTSTART;VALUE=DATE:20261015", end: "DTEND;VALUE=DATE:99991231", ..Default::default() })),
        ("thousands of people", odd(Meeting { more: people, ..Default::default() })),
        ("a name of megabytes", odd(Meeting { uid: "u".repeat(1_500_000), ..Default::default() })),
        ("a name with a line in it", odd(Meeting { uid: "uid\\nBEGIN:VEVENT".into(), ..Default::default() })),
        ("no name", odd(Meeting { uid: String::new(), ..Default::default() })),
        ("a method that is none", odd(Meeting { method: "DROP TABLE; \u{202e}", ..Default::default() })),
        ("no method", carrying("No method", b"BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:x\r\nDTSTART:20261015T140000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n".to_vec())),
        ("a rule that never rests", odd(Meeting { more: "RRULE:FREQ=DAILY;INTERVAL=4294967295;COUNT=4294967295;BYDAY=MO,XX".into(), ..Default::default() })),
        ("an occurrence of nothing", odd(Meeting { more: "RECURRENCE-ID:99999999T999999Z".into(), ..Default::default() })),
        ("an occurrence", odd(Meeting { more: "RECURRENCE-ID:20261015T140000Z".into(), ..Default::default() })),
        ("nobody organizes it", odd(Meeting { organizer: "", ..Default::default() })),
        ("an organizer that is no address", odd(Meeting { organizer: "ORGANIZER;CN=\u{0}\u{7}:mailto:\r\n \u{202e}", ..Default::default() })),
        (
            "a title and a place of half a megabyte each",
            carrying(
                "Long",
                format!(
                    "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:uid-long\r\nSUMMARY:{}\r\nLOCATION:{}\r\nDESCRIPTION:{}\r\nDTSTART:20261015T140000Z\r\nDTEND:20261015T150000Z\r\nORGANIZER;CN={}:mailto:bo@example.org\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
                    "T".repeat(500_000),
                    "L\u{202e}".repeat(120_000),
                    "D".repeat(400_000),
                    "N".repeat(100_000),
                )
                .into_bytes(),
            ),
        ),
        ("bytes that are no text", carrying("Bytes", (0u32..4_000).map(|n| (n.wrapping_mul(2_654_435_761) >> 13) as u8).collect())),
        ("larger than is read", carrying("Large", format!("{}{}", Meeting::default().ics(), "X-PAD:x\r\n".repeat(300_000)).into_bytes())),
        ("a cancellation of nothing", odd(Meeting { method: "CANCEL", uid: "uid-nothing".into(), ..Default::default() })),
        ("an answer from nobody", odd(Meeting { method: "REPLY", ..Default::default() })),
        ("a counter", odd(Meeting { method: "COUNTER", ..Default::default() })),
    ]
}

#[test]
fn hostile_invitations_never_panic_and_never_offer_a_button_that_cannot_work() {
    for (theme, size) in [("light", (1280.0, 900.0)), ("dark", (960.0, 640.0))] {
        let named = hostile();
        let (mut app, account, ids) = mail_app(named.iter().map(|(_, m)| m.clone()).collect());
        app.execute("ui.theme", json!({"value": theme})).unwrap();
        let mut harness = window(app, size);
        for ((name, _), id) in named.iter().zip(&ids) {
            select(&mut harness, *id);
            harness.run_steps(3);
            let shown = ANSWERS.iter().any(|kind| hit(&harness, kind, *id).is_some());
            let meeting = harness.state_mut().execute("calendar.invitation", json!({"message": id}));
            assert_eq!(shown, meeting.as_ref().is_ok_and(|m| m["answerable"] == true), "{name}");
            if !shown {
                continue;
            }
            // What is offered works: each answer, sent or not. Where the organizer can't be
            // written to, no question about that is asked, and nothing is sent.
            let can_send = meeting.is_ok_and(|m| m["canSend"] == true);
            let sent = outbox(harness.state(), account).len();
            for (kind, send) in [("invitationTentative", false), ("invitationAccept", true), ("invitationDecline", true)] {
                press(&mut harness, kind, *id);
                assert_eq!(asked(&harness).is_some(), can_send, "{name}: {kind}");
                if can_send {
                    answer(&mut harness, if send { "Send the Response Now" } else { "Don't Send a Response" });
                }
                let status = harness.state().status_msg.as_ref().map(|(text, _)| text.clone());
                assert!(harness.state().dialog.is_none() && status.is_none(), "{name}: {kind} said {status:?}");
                assert!(harness.state().session.mailbox.message(*id).is_some_and(|m| m.answered), "{name}: {kind} did nothing");
                harness.run_steps(2);
            }
            assert_eq!(outbox(harness.state(), account).len(), sent + if can_send { 2 } else { 0 }, "{name}");
            // The Calendar button too, and back to the mail.
            if hit(&harness, "invitationCalendar", *id).is_some() {
                press(&mut harness, "invitationCalendar", *id);
                harness.run_steps(2);
                harness.state_mut().execute("view.mail", json!({})).unwrap();
                harness.run_steps(2);
            }
        }
        // Each of them in a window of its own as well.
        for id in ids.iter().take(12) {
            harness.state_mut().execute("ui.openWindow", json!({"message": id})).unwrap();
        }
        harness.run_steps(6);
        harness.state_mut().execute("ui.closeWindow", json!({"all": true})).unwrap();
        harness.run_steps(2);
    }
}

#[test]
fn nothing_about_a_meeting_is_asked_of_anybody_without_the_owner() {
    // Every kind of meeting mail, opened and read, on an account with a mail server and on a
    // Microsoft account with nothing on its calendar: no mail is made, and the calendar's server
    // hears nothing but questions.
    let all = || {
        let mut all: Vec<Message> = hostile().into_iter().map(|(_, m)| m).collect();
        all.extend([Meeting::default().mail(), Meeting { method: "CANCEL", ..Default::default() }.mail()]);
        all
    };
    let (app, account, ids) = mail_app(all());
    let before = app.session.mailbox.clone();
    let mut harness = window(app, (1280.0, 900.0));
    for id in &ids {
        select(&mut harness, *id);
    }
    assert!(outbox(harness.state(), account).is_empty());
    assert!(harness.state().session.mailbox.events == before.events && harness.state().session.mailbox.event_changes.is_empty());

    let (mut app, server, account) = microsoft(vec![invited("g-1", "uid-design-review", THURSDAY)]);
    let ids = deliver(&mut app, account, all());
    let mut harness = window(app, (1280.0, 900.0));
    for id in &ids {
        select(&mut harness, *id);
    }
    let server = server.lock().unwrap();
    assert!(server.mail.is_empty() && server.jobs.iter().all(|job| job.ops.is_empty()), "{:?}", server.jobs);
    assert!(outbox(harness.state(), account).is_empty() && harness.state().session.mailbox.event_changes.is_empty());
}

#[test]
fn the_bars_command_takes_anything_and_never_panics() {
    let (mut app, _, ids) = mail_app(vec![Meeting::default().mail()]);
    for junk in [
        json!(null),
        json!({}),
        json!([]),
        json!("x"),
        json!({"message": -1}),
        json!({"message": "x"}),
        json!({"message": 1e300}),
        json!({"message": ids[0]}),
    ] {
        let _ = app.execute("ui.findInvitation", junk.clone());
        let _ = app.run("ui.findInvitation", junk.clone());
        let _ = app.run("calendar.respond", junk.clone());
        let _ = app.run("calendar.removeCancelled", junk.clone());
        let _ = app.run("calendar.showInvitation", junk);
        app.dialog = None;
    }
    // A question that is asked of a message that is no invitation: none, and the command says why.
    let plain = deliver(&mut app, 1, vec![super::message(1, ("Jo", "jo@example.org"), "Lunch?", None, "Lunch?".into())])[0];
    assert!(app.run("calendar.respond", json!({"message": plain, "response": "accept"})).is_err() && app.dialog.is_none());
}

/// The bar for each kind of meeting mail, in light and dark, as PNG files to look at:
/// `MAILCRAFT_SHOTS_DIR=<dir> cargo test -p mailcraft-ui-egui shots_of_meeting_mail -- --ignored`
#[test]
#[ignore = "writes screenshots, for looking at them"]
fn shots_of_meeting_mail() {
    let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") else {
        eprintln!("set MAILCRAFT_SHOTS_DIR to the directory to write to");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    for theme in ["light", "dark"] {
        let series = Meeting { uid: "uid-series".into(), title: "Weekly planning", more: "RRULE:FREQ=WEEKLY;BYDAY=TH".into(), ..Default::default() };
        let update = Meeting { uid: "uid-update".into(), title: "Budget review", more: "SEQUENCE:2".into(), ..Default::default() };
        let cancel = Meeting { method: "CANCEL", uid: "uid-update".into(), title: "Budget review", ..Default::default() };
        let reply = Meeting {
            method: "REPLY",
            uid: "uid-mine".into(),
            title: "Roadmap",
            organizer: "ORGANIZER;CN=Avery Quinn:mailto:avery@mail.example",
            more: "ATTENDEE;PARTSTAT=ACCEPTED;CN=Maya Okafor:mailto:maya@example.org".into(),
            ..Default::default()
        };
        let past = Meeting {
            uid: "uid-past".into(),
            title: "Kick-off",
            start: "DTSTART:20261001T140000Z",
            end: "DTEND:20261001T150000Z",
            ..Default::default()
        };
        let (mut app, _, ids) = mail_app(vec![Meeting::default().mail(), series.mail(), update.mail(), cancel.mail(), reply.mail(), past.mail()]);
        app.execute("ui.theme", json!({"value": theme})).unwrap();
        app.execute("calendar.new", json!({"title": "Dentist", "start": "2026-10-15T14:30", "end": "2026-10-15T15:30"})).unwrap();
        app.execute("calendar.new", json!({"title": "Stand-up", "start": "2026-10-15T13:30", "end": "2026-10-15T14:00"})).unwrap();
        let mut harness = egui_kittest::Harness::builder()
            .with_accessibility_check(false)
            .allow_missing_glyphs()
            .with_size(egui::vec2(1440.0, 900.0))
            .with_pixels_per_point(2.0)
            .with_step_dt(1.0 / 60.0)
            .wgpu()
            .build_ui_state(
                |ui, app: &mut MailApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                app,
            );
        harness.input_mut().max_texture_side = Some(8192);
        harness.run_steps(4);
        let shot = |harness: &mut Window, name: &str| {
            harness.run_steps(10);
            let path = dir.join(format!("{name}-{theme}.png"));
            harness.render().unwrap().save(&path).unwrap();
            println!("{}", path.display());
        };
        select(&mut harness, ids[0]);
        shot(&mut harness, "meeting-1-request");
        press(&mut harness, "invitationAcceptMenu", ids[0]);
        shot(&mut harness, "meeting-2-menu");
        harness.key_press(egui::Key::Escape);
        press(&mut harness, "invitationAccept", ids[0]);
        shot(&mut harness, "meeting-3-question");
        answer(&mut harness, "Don't Send a Response");
        shot(&mut harness, "meeting-4-accepted");
        select(&mut harness, ids[1]);
        shot(&mut harness, "meeting-5-series");
        select(&mut harness, ids[2]);
        shot(&mut harness, "meeting-6-update");
        harness.state_mut().execute("calendar.respond", json!({"message": ids[2], "response": "tentative"})).unwrap();
        select(&mut harness, ids[3]);
        shot(&mut harness, "meeting-7-cancellation");
        select(&mut harness, ids[4]);
        shot(&mut harness, "meeting-8-reply");
        select(&mut harness, ids[5]);
        shot(&mut harness, "meeting-9-past");
        harness.state_mut().execute("ui.openWindow", json!({"message": ids[1]})).unwrap();
        shot(&mut harness, "meeting-10-window");
        harness.state_mut().execute("ui.closeWindow", json!({"all": true})).unwrap();
        harness.state_mut().execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
        shot(&mut harness, "meeting-11-options");
    }
}

#[test]
fn a_request_offers_to_propose_another_time_and_sends_only_from_its_dialog() {
    let closed = Meeting { uid: "uid-closed".into(), title: "No proposals", more: "X-MICROSOFT-DISALLOW-COUNTER:TRUE".into(), ..Meeting::default() };
    let (app, account, ids) = mail_app(vec![Meeting::default().mail(), closed.mail()]);
    let (request, shut) = (ids[0], ids[1]);
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, request);
    // Beside the three answers: Propose New Time, which opens a dialog and sends nothing.
    assert!(hit(&harness, "invitationPropose", request).is_some());
    press(&mut harness, "invitationPropose", request);
    harness.run_steps(3);
    assert_eq!(harness.state().dialog.as_ref().map(|d| d.name()), Some("proposeTime"));
    assert!(says(&harness, "Bo Marsh organizes \"Design review\"."));
    assert!(says(&harness, "It is now on Thursday, October 15, 2026 2:00 PM – 3:00 PM."));
    assert!(says(&harness, "Either button writes to Bo Marsh"));
    assert!(outbox(harness.state(), account).is_empty() && events(harness.state(), "Design review").is_empty());
    // Escape: nobody was written to, and nothing was answered.
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);
    assert!(harness.state().dialog.is_none() && outbox(harness.state(), account).is_empty());
    assert_eq!(said(&mut harness, request)["answered"], Value::Null);
    // Tentative and Propose, pressed: the meeting is on the calendar, tentatively, and the
    // proposal waits in the Outbox for its organizer, as a counter-proposal his program reads.
    press(&mut harness, "invitationPropose", request);
    harness.run_steps(3);
    let button = harness.state().hit.rect("proposeTentative", 0).unwrap();
    click_at(&mut harness, button.center());
    harness.run_steps(4);
    assert!(harness.state().dialog.is_none());
    let waiting = outbox(harness.state(), account);
    assert_eq!(waiting.len(), 1);
    assert_eq!((waiting[0].0.as_str(), waiting[0].1.as_str()), ("bo@example.org", "Another time proposed: Design review"));
    assert!(waiting[0].2.contains("METHOD:COUNTER") && waiting[0].2.contains("UID:uid-design-review"), "{}", waiting[0].2);
    assert_eq!(events(harness.state(), "Design review").len(), 1);
    assert!(says(&harness, "Tentatively accepted on"));
    // An organizer who takes no proposals: nothing to press.
    select(&mut harness, shut);
    assert!(hit(&harness, "invitationAccept", shut).is_some() && hit(&harness, "invitationPropose", shut).is_none());
}

#[test]
fn a_message_that_proposes_another_time_offers_to_take_it_up() {
    // The owner's own meeting, kept on this computer, with Bo invited; and Bo's proposal of
    // another time for it, as his program mails one.
    let counter = Meeting {
        method: "COUNTER",
        uid: "uid-planning".into(),
        title: "Planning",
        start: "DTSTART:20261016T150000Z",
        end: "DTEND:20261016T163000Z",
        organizer: "ORGANIZER;CN=Avery Quinn:mailto:avery@mail.example",
        more: String::new(),
    };
    let (mut app, _, ids) = mail_app(vec![counter.mail(), Meeting::default().mail()]);
    let calendar = app.execute("calendar.addCalendar", json!({"name": "Mine"})).unwrap()["id"].as_u64().unwrap();
    let at = |day: u8, hour: u8| mailcraft_time::DateTime::new(mailcraft_time::Date::new(2026, 10, day).unwrap(), hour, 0, 0).unwrap();
    let planning = mailcraft_model::Event {
        calendar,
        uid: "uid-planning".into(),
        title: "Planning".into(),
        start: at(15, 10),
        end: at(15, 11),
        organizer: Some(Address::new("Avery Quinn", OWNER)),
        attendees: vec![Attendee { address: Address::new("Bo Marsh", "bo@example.org"), optional: false, response: Response::Tentative }],
        ..Default::default()
    };
    let planning = app.session.mailbox.add_event(planning).unwrap();
    app.session.mailbox_changed();
    let (proposal, request) = (ids[0], ids[1]);
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, proposal);
    // What it is, what is proposed, when the meeting is now; and something to press.
    assert!(says(&harness, "proposes another time for this meeting."));
    assert!(says(&harness, "Proposed Friday, October 16, 2026 3:00 PM – 4:30 PM"));
    assert!(hit(&harness, "invitationAcceptProposal", proposal).is_some() && hit(&harness, "invitationAccept", proposal).is_none());
    // One proposal is all there is: no list to look at.
    assert!(hit(&harness, "invitationProposals", proposal).is_none());
    // Accept Proposal: the meeting opens with the proposed time in its fields. Nothing is
    // changed, and nothing is sent, until the owner says so there.
    press(&mut harness, "invitationAcceptProposal", proposal);
    harness.run_steps(4);
    let ctx = harness.ctx.clone();
    let open = crate::control::inspect(harness.state_mut(), &ctx)["windows"].clone();
    assert_eq!((&open[0]["event"], &open[0]["dirty"]), (&json!(planning), &json!(true)), "{open}");
    assert_eq!(harness.state().session.mailbox.event(planning).map(|e| e.start), Some(at(15, 10)));
    // Saved from there, it is at the proposed time.
    let window = open[0]["id"].as_u64().unwrap();
    harness.state_mut().execute("ui.eventWindow", json!({"window": window, "do": "saveAndClose"})).unwrap();
    harness.run_steps(6);
    let now = harness.state().session.mailbox.event(planning).cloned().unwrap();
    assert_eq!(
        (now.start, now.end),
        (at(16, 15), mailcraft_time::DateTime::new(mailcraft_time::Date::new(2026, 10, 16).unwrap(), 16, 30, 0).unwrap())
    );
    // A second proposal that the calendar knows of: now there is a list, which opens.
    harness.state_mut().session.mailbox.event_mut(planning).unwrap().proposals =
        vec![mailcraft_model::Proposal { from: Address::new("Cy Tran", "cy@example.org"), start: at(17, 9), end: at(17, 10) }];
    harness.state_mut().session.mailbox_changed();
    harness.run_steps(3);
    assert!(hit(&harness, "invitationProposals", proposal).is_some());
    press(&mut harness, "invitationProposals", proposal);
    harness.run_steps(3);
    assert_eq!(harness.state().dialog.as_ref().map(|d| d.name()), Some("proposals"));
    assert!(
        says(&harness, "Bo Marsh: Friday, October 16, 2026 3:00 PM – 4:30 PM")
            && says(&harness, "Cy Tran: Saturday, October 17, 2026 9:00 AM – 10:00 AM")
    );
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);
    // A request has none of this.
    select(&mut harness, request);
    assert!(hit(&harness, "invitationAcceptProposal", request).is_none() && hit(&harness, "invitationProposals", request).is_none());
}
