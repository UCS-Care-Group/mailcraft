// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Importance, response options, Teams meetings, proposing another time, forwarding, and moving
//! an appointment between the calendars of one account, against a server written out by hand (as
//! `mailcraft-net` brings it from Microsoft Graph). Invented people at `example.org`.

use mailcraft_model::remote::{
    CalendarsDownload, EventChangeKind, EventFields, EventOp, EventOutcome, EventResult, EventsDownload, EventsUpload, EventsUploaded,
    RemoteCalendar, RemoteEvent, RemoteProposal,
};
use mailcraft_model::{
    Address, Attachment, Attendee, Event, FolderKind, Freq, Id, Importance, Message, OnlineMeeting, OnlineProvider, Recurrence, Response,
    ResponseOptions,
};
use mailcraft_time::{Date, DateTime, Timestamp};
use serde_json::{Value, json};

use crate::cmd::remote::{apply_calendars, apply_event_outcomes, apply_events, event_upload, events_waiting};
use crate::{CmdError, Session};

const JOIN: &str = "https://teams.example.org/l/meetup-join/19%3ameeting_abc%40thread.v2/0";

/// A time of day in October 2026, by a clock in UTC.
fn clock(day: u8, hour: u8, minute: u8) -> DateTime {
    DateTime::new(Date::new(2026, 10, day).unwrap(), hour, minute, 0).unwrap()
}

fn at(day: u8, hour: u8, minute: u8) -> Timestamp {
    clock(day, hour, minute).to_timestamp(0)
}

/// The sample mailbox on Wednesday 2026-10-14, 16:00 UTC, with an account that signs in with
/// Microsoft (`ada@example.org`), whose server keeps two calendars: its main one and "Side".
/// The account, the main calendar and the other one.
fn session() -> (Session, Id, Id, Id) {
    let now = at(14, 16, 0);
    let mut s = Session::at(crate::sample::mailbox(now, 0), now, 0);
    let added =
        s.run("account.add", &json!({"email": "ada@example.org", "name": "Ada", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"}));
    let account = added.unwrap()["id"].as_u64().unwrap();
    let list = CalendarsDownload {
        account,
        email: "ada@example.org".into(),
        complete: true,
        calendars: vec![
            RemoteCalendar { id: "c-main".into(), name: "Calendar".into(), main: true, ..Default::default() },
            RemoteCalendar { id: "c-side".into(), name: "Side".into(), ..Default::default() },
        ],
        ..Default::default()
    };
    apply_calendars(&mut s, &list).unwrap();
    let calendar = |name: &str| s.mailbox.calendars.iter().find(|c| c.origin.account == account && c.origin.id == name).map(|c| c.id).unwrap();
    let (main, side) = (calendar(""), calendar("c-side"));
    (s, account, main, side)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// Why a command was refused.
fn refused(s: &mut Session, id: &str, params: Value) -> String {
    match s.run(id, &params) {
        Err(e @ (CmdError::Failed(_) | CmdError::Said(_))) => e.to_string(),
        other => panic!("{id} {params}: expected a refusal, got {other:?}"),
    }
}

/// An hour of the owner's own as the server has it.
fn remote(id: &str, title: &str, start: Timestamp) -> RemoteEvent {
    RemoteEvent {
        id: id.into(),
        change_key: format!("ck-{id}"),
        title: title.into(),
        start,
        end: start + 3600,
        is_organizer: Some(true),
        ..Default::default()
    }
}

fn invite(name: &str, email: &str) -> Attendee {
    Attendee { address: Address::new(name, email), ..Default::default() }
}

/// A meeting the owner organizes, with Bo and Cy invited.
fn own_meeting(id: &str, title: &str, start: Timestamp) -> RemoteEvent {
    RemoteEvent {
        organizer: Some(Address::new("Ada", "ada@example.org")),
        attendees: vec![invite("Bo", "bo@example.org"), invite("Cy", "cy@example.org")],
        ..remote(id, title, start)
    }
}

/// Bo's meeting, to which the owner is invited.
fn their_meeting(id: &str, title: &str, start: Timestamp) -> RemoteEvent {
    RemoteEvent {
        organizer: Some(Address::new("Bo", "bo@example.org")),
        attendees: vec![invite("Ada", "ada@example.org"), invite("Cy", "cy@example.org")],
        is_organizer: Some(false),
        uid: format!("uid-{id}"),
        ..remote(id, title, start)
    }
}

/// All of October of one of the account's calendars (`""` is its main one), with everything said.
fn october(account: Id, calendar: &str, events: Vec<RemoteEvent>) -> EventsDownload {
    EventsDownload {
        account,
        email: "ada@example.org".into(),
        calendar: calendar.into(),
        from: at(1, 0, 0),
        to: at(31, 23, 59),
        complete: true,
        events,
        has_categories: true,
        has_options: true,
        ..Default::default()
    }
}

fn serve(s: &mut Session, account: Id, calendar: &str, events: Vec<RemoteEvent>) {
    apply_events(s, &october(account, calendar, events)).unwrap();
}

fn event_called(s: &Session, remote: &str) -> Event {
    s.mailbox.events.values().find(|e| e.origin.id == remote).map(|e| e.as_ref().clone()).unwrap_or_else(|| panic!("no event {remote}"))
}

/// What the server is to be told now.
fn ops(s: &Session, account: Id) -> Vec<EventOp> {
    event_upload(s, account).unwrap().ops
}

/// The answer of a server that said `result` to everything: a new event is `made-` and the number the event has here.
fn said(upload: &EventsUpload, result: EventResult, error: &str) -> EventsUploaded {
    let outcome = |op: &EventOp| EventOutcome {
        op: op.clone(),
        result,
        remote: match (result, op.kind) {
            (EventResult::Done, EventChangeKind::Create) => format!("made-{}", op.event),
            (EventResult::Done, _) => op.remote.clone(),
            _ => String::new(),
        },
        change_key: if result == EventResult::Done { "ck-new".into() } else { String::new() },
        error: error.into(),
        ..Default::default()
    };
    EventsUploaded { account: upload.account, email: upload.email.clone(), outcomes: upload.ops.iter().map(outcome).collect(), ..Default::default() }
}

/// Send what waits to a server that says `result` to all of it, and take its answer in.
fn push(s: &mut Session, account: Id, result: EventResult, error: &str) -> crate::cmd::remote::EventsApplied {
    let upload = event_upload(s, account).unwrap();
    apply_event_outcomes(s, &said(&upload, result, error)).unwrap()
}

const UPDATE: &str = "2 people are invited to this meeting, and changing it makes the account's server send them an update by mail. Nothing was changed. Say that this is meant with \"send\": true.";

#[test]
fn a_mailbox_file_from_before_reads_as_it_was_and_writes_nothing_new() {
    // An event as it was written before any of this was kept: none of it is in the file.
    let lunch = json!({"id": 7, "calendar": 2, "title": "Lunch", "location": "Canteen", "allDay": false, "showAs": "tentative", "private": true});
    let event: Event = serde_json::from_value(lunch.clone()).unwrap();
    assert_eq!((event.title.as_str(), event.show_as, event.private), ("Lunch", mailcraft_model::ShowAs::Tentative, true));
    assert_eq!(
        (event.importance, event.response_options, &event.online, event.proposals.len(), event.has_attachments),
        (Importance::Normal, ResponseOptions { request_responses: true, allow_proposals: true }, &None, 0, false)
    );
    // And it is written as it was: nothing is said of what is as it always was.
    let written = serde_json::to_value(&event).unwrap();
    for name in ["importance", "responseOptions", "online", "proposals", "hasAttachments"] {
        assert!(written.get(name).is_none(), "{name} in {written}");
    }
    for (name, value) in lunch.as_object().unwrap() {
        assert_eq!(&written[name], value, "{name}");
    }
    // The whole sample mailbox, through its file and back.
    let s = Session::at(crate::sample::mailbox(at(14, 16, 0), 0), at(14, 16, 0), 0);
    let file = serde_json::to_string(&s.mailbox).unwrap();
    assert!(
        !file.contains("responseOptions") && !file.contains("onlineProviders") && !file.contains("\"replaces\""),
        "nothing new in a mailbox that has none of it"
    );
    assert!(serde_json::from_str::<mailcraft_model::Mailbox>(&file).unwrap() == s.mailbox);
    // What is said, comes back.
    let said = Event {
        importance: Importance::High,
        response_options: ResponseOptions { request_responses: false, allow_proposals: true },
        online: Some(OnlineMeeting { provider: OnlineProvider::Teams, join_url: JOIN.into() }),
        ..event
    };
    let written = serde_json::to_string(&said).unwrap();
    assert!(
        written.contains(r#""importance":"high""#) && written.contains(r#""responseOptions":{"requestResponses":false,"allowProposals":true}"#),
        "{written}"
    );
    assert!(written.contains(&format!(r#""online":{{"provider":"teams","joinUrl":"{JOIN}"}}"#)), "{written}");
    assert_eq!(serde_json::from_str::<Event>(&written).unwrap(), said);
}

#[test]
fn importance_and_response_options_go_both_ways() {
    let (mut s, account, main, _) = session();
    let planning = RemoteEvent {
        importance: Importance::High,
        options: ResponseOptions { request_responses: false, allow_proposals: false },
        ..own_meeting("m-1", "Planning", at(12, 13, 0))
    };
    serve(&mut s, account, "", vec![planning.clone(), remote("e-1", "Eye test", at(13, 9, 0)), their_meeting("g-1", "Board", at(15, 13, 0))]);
    let (own, dentist, board) = (event_called(&s, "m-1"), event_called(&s, "e-1"), event_called(&s, "g-1"));
    assert_eq!((own.importance, own.response_options), (Importance::High, ResponseOptions { request_responses: false, allow_proposals: false }));
    assert_eq!((dentist.importance, dentist.response_options), (Importance::Normal, ResponseOptions::default()));
    let got = run(&mut s, "calendar.get", json!({"id": own.id}));
    assert_eq!(
        (&got["importance"], &got["requestResponses"], &got["allowProposals"], &got["online"]),
        (&json!("high"), &json!(false), &json!(false), &Value::Null)
    );

    // Of the owner's own appointment: no word is needed, and only what changed is told.
    assert_eq!(run(&mut s, "calendar.importance", json!({"id": dentist.id, "value": "high"}))["importance"], "high");
    let told = ops(&s, account);
    assert_eq!((told.len(), &told[0].fields), (1, &EventFields { importance: Some(Importance::High), ..Default::default() }));
    // Back to the usual before the server heard: nothing is left to tell.
    run(&mut s, "calendar.importance", json!({"id": dentist.id, "value": "normal"}));
    assert!(ops(&s, account).is_empty());
    run(&mut s, "calendar.edit", json!({"id": dentist.id, "importance": "low"}));
    assert_eq!(ops(&s, account)[0].fields.importance, Some(Importance::Low));
    push(&mut s, account, EventResult::Done, "");
    assert!(s.mailbox.event_changes.is_empty());

    // Of a meeting the owner organizes, the people invited are told: only on the owner's yes.
    let snapshot = s.mailbox.clone();
    for (command, params) in [
        ("calendar.importance", json!({"id": own.id, "value": "low"})),
        ("calendar.responseOptions", json!({"id": own.id, "requestResponses": true})),
        ("calendar.responseOptions", json!({"id": own.id, "allowProposals": true, "send": false})),
        ("calendar.edit", json!({"id": own.id, "importance": "normal"})),
        ("calendar.edit", json!({"id": own.id, "allowProposals": true})),
    ] {
        assert_eq!(refused(&mut s, command, params.clone()), UPDATE, "{command} {params}");
        assert!(s.mailbox == snapshot, "{command} {params}");
    }
    let said = run(&mut s, "calendar.responseOptions", json!({"id": own.id, "requestResponses": true, "allowProposals": true, "send": true}));
    assert_eq!((&said["requestResponses"], &said["allowProposals"]), (&json!(true), &json!(true)));
    run(&mut s, "calendar.importance", json!({"id": own.id, "value": "low", "send": true}));
    let told = ops(&s, account);
    assert_eq!(told.len(), 1);
    assert_eq!(
        told[0].fields,
        EventFields { importance: Some(Importance::Low), request_responses: Some(true), allow_proposals: Some(true), ..Default::default() }
    );
    assert!(s.mailbox.event_changes[0].send);
    // A mailbox file that says the change is to go, and not that the owner said yes: it doesn't.
    s.mailbox.event_changes[0].send = false;
    assert!(ops(&s, account).is_empty());
    s.mailbox.event_changes[0].send = true;
    push(&mut s, account, EventResult::Done, "");

    // Somebody else's meeting: how much it matters to the owner is the owner's own business; what
    // the people invited are asked for is the organizer's.
    run(&mut s, "calendar.importance", json!({"id": board.id, "value": "high"}));
    assert_eq!(ops(&s, account)[0].fields, EventFields { importance: Some(Importance::High), ..Default::default() });
    let theirs = "This meeting is kept by the server of Ada, and somebody else organizes it. What the people invited are asked for is for its organizer to say.";
    assert_eq!(refused(&mut s, "calendar.responseOptions", json!({"id": board.id, "allowProposals": false})), theirs);
    assert_eq!(
        refused(&mut s, "calendar.responseOptions", json!({"id": dentist.id, "allowProposals": false})),
        "Response options are for a meeting: invite somebody first."
    );
    push(&mut s, account, EventResult::Done, "");

    // A new one goes with what it has of this, and says nothing of what is as usual.
    let made = run(&mut s, "calendar.new", json!({"title": "Flight", "start": "2026-10-20T09:00", "importance": "high", "calendar": main}));
    let plain = run(&mut s, "calendar.new", json!({"title": "Walk", "start": "2026-10-20T12:00", "calendar": main}));
    let told = ops(&s, account);
    let fields = |id: &Value| told.iter().find(|op| Some(op.event) == id.as_u64()).map(|op| op.fields.clone()).unwrap();
    assert_eq!(fields(&made["id"]).importance, Some(Importance::High));
    let walk = fields(&plain["id"]);
    assert_eq!((walk.importance, walk.request_responses, walk.allow_proposals, walk.online), (None, None, None, None));
    push(&mut s, account, EventResult::Done, "");

    // A download that says what they are is what they are; one that doesn't leaves them alone.
    let now = RemoteEvent { importance: Importance::Normal, options: ResponseOptions::default(), change_key: "ck-9".into(), ..planning.clone() };
    let silent = EventsDownload { has_options: false, events: vec![now.clone()], complete: false, ..october(account, "", Vec::new()) };
    apply_events(&mut s, &silent).unwrap();
    assert_eq!(event_called(&s, "m-1").importance, Importance::Low, "as it is here");
    apply_events(&mut s, &EventsDownload { complete: false, ..october(account, "", vec![now]) }).unwrap();
    let own = event_called(&s, "m-1");
    assert_eq!((own.importance, own.response_options), (Importance::Normal, ResponseOptions::default()));

    // What isn't one of the three is refused as a parameter, and nothing changes.
    for params in [json!({"id": dentist.id}), json!({"id": dentist.id, "value": "urgent!!"}), json!({"id": dentist.id, "value": 7})] {
        assert!(matches!(s.run("calendar.importance", &params), Err(CmdError::Params(_))), "{params}");
    }
    assert!(matches!(s.run("calendar.edit", &json!({"id": dentist.id, "importance": "huge"})), Err(CmdError::Params(_))));
    assert!(matches!(s.run("calendar.responseOptions", &json!({"id": own.id})), Err(CmdError::Params(_))));
}

#[test]
fn a_teams_meeting_is_made_by_the_server_and_comes_back_with_its_link() {
    let (mut s, account, main, _) = session();
    let local = s.mailbox.calendars.iter().find(|c| c.origin.is_local()).map(|c| c.id).unwrap();
    let meeting =
        json!({"title": "Sync", "start": "2026-10-20T10:00", "attendees": "bo@example.org", "online": true, "calendar": main, "send": true});
    let made = run(&mut s, "calendar.newMeeting", meeting.clone())["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.event(made).unwrap().online, Some(OnlineMeeting::default()));
    // Until the server has made it there is nothing to join by.
    assert_eq!(refused(&mut s, "calendar.join", json!({"id": made})), "The account's server hasn't given this meeting a link to join by yet.");
    let upload = event_upload(&s, account).unwrap();
    assert_eq!((upload.ops.len(), upload.ops[0].fields.online), (1, Some(OnlineProvider::Teams)));
    let mut answer = said(&upload, EventResult::Done, "");
    (answer.outcomes[0].online, answer.outcomes[0].join_url) = (Some(true), JOIN.into());
    let applied = apply_event_outcomes(&mut s, &answer).unwrap();
    assert!(applied.messages.is_empty() && s.mailbox.event_changes.is_empty(), "{applied:?}");
    assert_eq!(s.mailbox.event(made).unwrap().online, Some(OnlineMeeting { provider: OnlineProvider::Teams, join_url: JOIN.into() }));
    assert_eq!(run(&mut s, "calendar.join", json!({"id": made})), json!({"id": made, "url": JOIN, "provider": "teams"}));
    assert_eq!(run(&mut s, "calendar.get", json!({"id": made}))["online"], json!({"provider": "teams", "joinUrl": JOIN}));

    // Once the server has made it, it stays one: taking it back is refused, by every way of asking.
    let stays = "The account's server has made this a Teams meeting, and it stays one: that can't be taken back from here.";
    let snapshot = s.mailbox.clone();
    for (command, params) in [
        ("calendar.teamsMeeting", json!({"id": made, "send": true})),
        ("calendar.teamsMeeting", json!({"id": made, "value": false, "send": true})),
        ("calendar.edit", json!({"id": made, "online": false, "send": true})),
    ] {
        assert_eq!(refused(&mut s, command, params.clone()), stays, "{command} {params}");
        assert!(s.mailbox == snapshot);
    }
    // Asked for what it is already: nothing to tell anybody.
    run(&mut s, "calendar.teamsMeeting", json!({"id": made, "value": true}));
    assert!(s.mailbox.event_changes.is_empty());

    // A server that won't have it: the meeting is made without, and that is said plainly.
    let other = run(
        &mut s,
        "calendar.newMeeting",
        json!({"title": "Review", "start": "2026-10-21T10:00", "attendees": "bo@example.org", "online": true, "send": true}),
    );
    let other = other["id"].as_u64().unwrap();
    let upload = event_upload(&s, account).unwrap();
    let mut answer = said(&upload, EventResult::Done, "Microsoft Graph answered with status 400 (ErrorNotSupported)");
    answer.outcomes[0].online = Some(false);
    let applied = apply_event_outcomes(&mut s, &answer).unwrap();
    assert_eq!(
        applied.messages,
        [
            "\"Review\" couldn't be made a Teams meeting: the account's server wouldn't have it (Microsoft Graph answered with status 400 (ErrorNotSupported)). The meeting itself is saved, without a link to join."
        ]
    );
    let review = s.mailbox.event(other).unwrap().clone();
    assert_eq!((&review.online, review.origin.id.as_str(), review.attendees.len()), (&None, format!("made-{other}").as_str(), 1));
    assert!(s.mailbox.event_changes.is_empty(), "and nothing more is tried");
    assert_eq!(refused(&mut s, "calendar.join", json!({"id": other})), "This isn't an online meeting.");

    // Turned on for a meeting that is there: an update, on the owner's yes; the link comes back.
    let update = "1 person is invited to this meeting, and changing it makes the account's server send them an update by mail. Nothing was changed. Say that this is meant with \"send\": true.";
    assert_eq!(refused(&mut s, "calendar.teamsMeeting", json!({"id": other})), update);
    assert_eq!(run(&mut s, "calendar.teamsMeeting", json!({"id": other, "send": true}))["online"], true);
    let upload = event_upload(&s, account).unwrap();
    assert_eq!(upload.ops[0].fields, EventFields { online: Some(OnlineProvider::Teams), ..Default::default() });
    let mut answer = said(&upload, EventResult::Done, "");
    (answer.outcomes[0].online, answer.outcomes[0].join_url) = (Some(true), "javascript:alert(1)".into());
    apply_event_outcomes(&mut s, &answer).unwrap();
    // (What isn't an https address is no link: the meeting is online, with nothing to follow.)
    assert_eq!(s.mailbox.event(other).unwrap().online, Some(OnlineMeeting::default()));
    assert!(s.mailbox.event_changes.is_empty());

    // Before the server has heard of it, it can still be taken back.
    let third = run(
        &mut s,
        "calendar.newMeeting",
        json!({"title": "Maybe", "start": "2026-10-22T10:00", "attendees": "bo@example.org", "online": true, "send": true}),
    );
    run(&mut s, "calendar.teamsMeeting", json!({"id": third["id"], "value": false, "send": true}));
    assert_eq!(ops(&s, account)[0].fields.online, None);
    push(&mut s, account, EventResult::Done, "");

    // A meeting kept here that came with somebody's link to join by (an invitation from the
    // mail), put into the server's calendar: the server is not asked to make a meeting of its
    // own for it. That is only asked for a meeting that has no link yet.
    let theirs = Event {
        calendar: local,
        title: "Their call".into(),
        start: clock(23, 9, 0),
        end: clock(23, 10, 0),
        online: Some(OnlineMeeting { provider: OnlineProvider::Teams, join_url: JOIN.into() }),
        ..Default::default()
    };
    let theirs = s.mailbox.add_event(theirs).unwrap();
    run(&mut s, "calendar.edit", json!({"id": theirs, "calendar": main}));
    let told = ops(&s, account);
    assert_eq!((told.len(), told[0].kind, told[0].fields.online), (1, EventChangeKind::Create, None));
    push(&mut s, account, EventResult::Done, "");

    // A calendar on this computer has no server to make one; neither has one whose account can't.
    let kept_here = "A Teams meeting is made by an account's server, and this calendar is kept on this computer only.";
    assert_eq!(refused(&mut s, "calendar.new", json!({"title": "Solo", "online": true, "calendar": local})), kept_here);
    let solo = run(&mut s, "calendar.new", json!({"title": "Solo", "calendar": local}))["id"].clone();
    assert_eq!(refused(&mut s, "calendar.teamsMeeting", json!({"id": solo})), kept_here);
    assert_eq!(crate::cmd::calendar::teams_refusal(&s, local).as_deref(), Some(kept_here));
    assert_eq!(crate::cmd::calendar::teams_refusal(&s, main), None);
    // (A meeting that is kept here because nobody is to be invited yet is kept without it.)
    let quiet =
        run(&mut s, "calendar.newMeeting", json!({"title": "Quiet", "attendees": "bo@example.org", "online": true, "calendar": main, "send": false}));
    assert_eq!((quiet["keptHere"].as_bool(), &s.mailbox.event(quiet["id"].as_u64().unwrap()).unwrap().online), (Some(true), &None));
    s.mailbox.calendars.iter_mut().find(|c| c.id == main).unwrap().online_providers = Some(vec![OnlineProvider::Skype]);
    let cannot = "The account of the calendar \"Calendar\" can't make Teams meetings.";
    assert_eq!(refused(&mut s, "calendar.newMeeting", meeting), cannot);
    assert_eq!(crate::cmd::calendar::teams_refusal(&s, main).as_deref(), Some(cannot));

    // Somebody else's meeting: whether it is held online is the organizer's to say. Its link is
    // what its server says it is, and only an https address is kept of that.
    let online = |link: &str| Some(OnlineMeeting { provider: OnlineProvider::Teams, join_url: link.into() });
    serve(
        &mut s,
        account,
        "",
        vec![
            RemoteEvent { online: online(JOIN), ..their_meeting("g-1", "Board", at(15, 13, 0)) },
            RemoteEvent { online: online("http://teams.example.org/plain"), ..their_meeting("g-2", "Plain", at(16, 13, 0)) },
            RemoteEvent { online: online("https://user:pw@teams.example.org/x"), ..their_meeting("g-3", "Tricky", at(17, 13, 0)) },
            their_meeting("g-4", "Offline", at(18, 13, 0)),
        ],
    );
    let (first, offline) = (event_called(&s, "g-1").id, event_called(&s, "g-4").id);
    assert_eq!(run(&mut s, "calendar.join", json!({"id": first}))["url"], JOIN);
    for name in ["g-2", "g-3"] {
        assert_eq!(event_called(&s, name).online, online(""), "{name}");
    }
    let theirs = "This meeting is kept by the server of Ada, and somebody else organizes it. Whether it is held online is for its organizer to say.";
    assert_eq!(refused(&mut s, "calendar.teamsMeeting", json!({"id": offline, "value": true})), theirs);
}

#[test]
fn another_time_is_proposed_only_on_the_owners_word() {
    let (mut s, account, _, _) = session();
    let closed =
        RemoteEvent { options: ResponseOptions { request_responses: true, allow_proposals: false }, ..their_meeting("g-2", "Closed", at(16, 13, 0)) };
    serve(&mut s, account, "", vec![their_meeting("g-1", "Board", at(15, 13, 0)), closed, own_meeting("m-1", "Planning", at(12, 13, 0))]);
    let (board, shut, own) = (event_called(&s, "g-1").id, event_called(&s, "g-2").id, event_called(&s, "m-1").id);
    let snapshot = s.mailbox.clone();
    let times = json!({"start": "2026-10-16T13:00", "end": "2026-10-16T14:00"});
    let with = |more: Value| {
        let mut params = times.clone();
        params.as_object_mut().unwrap().extend(more.as_object().unwrap().clone());
        params
    };
    // It is mail to the organizer: nothing happens without the yes, and a no is no yes.
    let unasked = "Proposing another time writes to the meeting's organizer. Nothing was sent. Say that this is meant with \"send\": true.";
    for params in [with(json!({"id": board})), with(json!({"id": board, "send": false})), with(json!({"id": board, "send": "yes"}))] {
        assert_eq!(refused(&mut s, "calendar.propose", params.clone()), unasked, "{params}");
    }
    // Nor through the answer itself.
    let through = json!({"id": board, "response": "tentative", "propose": times.clone()});
    assert_eq!(refused(&mut s, "calendar.respond", through), unasked);
    // An organizer who takes no proposals gets none; a meeting of the owner's own has nobody to propose to.
    assert_eq!(
        refused(&mut s, "calendar.propose", with(json!({"id": shut, "send": true}))),
        "The organizer of this meeting doesn't take proposals of another time."
    );
    assert_eq!(
        refused(&mut s, "calendar.propose", with(json!({"id": own, "send": true}))),
        "You organize this meeting, so there is nobody to send an answer to."
    );
    // A plain yes proposes nothing; an end that isn't after its start is no time.
    let plain = json!({"id": board, "response": "accept", "send": true, "propose": times.clone()});
    assert_eq!(refused(&mut s, "calendar.respond", plain), "Another time is proposed with a tentative answer or with a decline.");
    let backwards = json!({"id": board, "send": true, "start": "2026-10-16T14:00", "end": "2026-10-16T13:00"});
    assert_eq!(refused(&mut s, "calendar.propose", backwards), "the proposed end is not after the proposed start");
    for params in [
        json!({"id": board, "send": true}),
        json!({"id": board, "send": true, "start": "soon", "end": 7}),
        with(json!({"id": board, "send": true, "response": "accept"})),
    ] {
        assert!(matches!(s.run("calendar.propose", &params), Err(CmdError::Params(_))), "{params}");
    }
    assert!(s.mailbox == snapshot, "none of it changed anything");

    // Tentative, and another time: the answer in the calendar, and both for the organizer.
    let answer = run(&mut s, "calendar.propose", with(json!({"id": board, "send": true, "comment": " Can you make Friday? "})));
    assert_eq!(
        answer,
        json!({"id": board, "response": "tentative", "sent": true, "proposed": {"start": "2026-10-16T13:00:00", "end": "2026-10-16T14:00:00"}})
    );
    let told = ops(&s, account);
    assert_eq!(told.len(), 1);
    assert_eq!(
        (told[0].kind, told[0].remote.as_str(), told[0].response, told[0].send, told[0].comment.as_str()),
        (EventChangeKind::Respond, "g-1", Response::Tentative, true, "Can you make Friday?")
    );
    assert_eq!(told[0].proposed.map(|time| (time.start, time.end, time.all_day)), Some((at(16, 13, 0), at(16, 14, 0), false)));
    assert_eq!(event_called(&s, "g-1").attendees[0].response, Response::Tentative);
    // The meeting stays where its organizer has it: a proposal is a proposal.
    assert_eq!(event_called(&s, "g-1").start, clock(15, 13, 0));
    // A mailbox file that says another time is proposed, and not that the owner said yes: it
    // doesn't go, not even as the plain answer it would otherwise be.
    s.mailbox.event_changes[0].send = false;
    assert!(ops(&s, account).is_empty());
    assert_eq!(events_waiting(&s, account).total, 0);
    s.mailbox.event_changes[0].send = true;
    assert_eq!(ops(&s, account).len(), 1);
    // The server won't (the organizer closed it meanwhile): the answer is put back, and says so.
    let applied = push(&mut s, account, EventResult::Refused, "Microsoft Graph answered with status 400");
    assert_eq!(
        applied.messages,
        ["The server wouldn't take your answer to \"Board\" (Microsoft Graph answered with status 400). It was put back as it was."]
    );
    assert_eq!(event_called(&s, "g-1").attendees[0].response, Response::None);

    // Declined, with another time: out of the calendar, as any meeting that is declined.
    let answer = run(&mut s, "calendar.propose", with(json!({"id": board, "send": true, "response": "decline"})));
    assert_eq!((&answer["response"], &answer["sent"]), (&json!("declined"), &json!(true)));
    assert!(s.mailbox.event(board).is_none());
    let told = ops(&s, account);
    assert_eq!((told[0].response, told[0].send, told[0].proposed.is_some()), (Response::Declined, true, true));
    push(&mut s, account, EventResult::Done, "");
    assert!(s.mailbox.event_changes.is_empty());

    // One occurrence of somebody else's series: the proposal is for that day.
    let weekly = RemoteEvent {
        kind: mailcraft_model::remote::RemoteEventKind::SeriesMaster,
        recurrence: Some(Recurrence::every(Freq::Weekly)),
        series_day: Date::new(2026, 10, 5),
        ..their_meeting("s-1", "Stand-up", at(5, 9, 0))
    };
    let occurrence = |day: u8| RemoteEvent {
        kind: mailcraft_model::remote::RemoteEventKind::Occurrence,
        series: "s-1".into(),
        original: Some(at(day, 9, 0)),
        ..their_meeting(&format!("s-1@{day}"), "Stand-up", at(day, 9, 0))
    };
    serve(&mut s, account, "", vec![weekly, occurrence(5), occurrence(12), occurrence(19), occurrence(26)]);
    let series = event_called(&s, "s-1").id;
    let one = json!({"id": series, "send": true, "scope": "occurrence", "occurrence": "2026-10-19", "start": "2026-10-19T11:00", "end": "2026-10-19T12:00"});
    run(&mut s, "calendar.propose", one);
    let told = ops(&s, account);
    assert_eq!(
        (told[0].kind, told[0].series.as_str(), told[0].occurrence, told[0].remote.as_str()),
        (EventChangeKind::Respond, "s-1", at(19, 9, 0), "")
    );
    assert_eq!(told[0].proposed.map(|time| time.start), Some(at(19, 11, 0)));

    // A meeting in a calendar on this computer answers nobody: there is nothing to propose with.
    let local = s.mailbox.events.values().find(|e| e.origin.is_local() && e.is_meeting()).map(|e| e.id).unwrap();
    assert_eq!(
        refused(&mut s, "calendar.propose", with(json!({"id": local, "send": true}))),
        "Another time can be proposed for a meeting that the account's server keeps, or from the invitation in the mail."
    );
}

/// A message from Bo in the Inbox of `account` with a calendar part.
fn mail(s: &mut Session, account: Id, method: &str, uid: &str, more: &str) -> Id {
    let times = if more.contains("DTSTART") { "" } else { "DTSTART:20261015T130000Z\r\nDTEND:20261015T140000Z\r\n" };
    let ics = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:{method}\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:Board\r\n{times}\
         ORGANIZER;CN=Bo Marsh:mailto:bo@example.org\r\nATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;CN=Ada:mailto:ada@example.org\r\n{more}\
         END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let message = Message {
        account,
        folder: inbox,
        message_id: format!("{}-{uid}@example.org", ics.len()),
        from: Address::new("Bo Marsh", "bo@example.org"),
        to: vec![Address::new("Ada", "ada@example.org")],
        subject: "Board".into(),
        attachments: vec![Attachment::new("invite.ics", "text/calendar", ics.into_bytes())],
        ..Default::default()
    };
    s.mailbox.add_message(message).unwrap()
}

fn outbox(s: &Session, account: Id) -> Vec<Message> {
    let folder = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    s.mailbox.messages.values().filter(|m| m.folder == folder).map(|m| m.as_ref().clone()).collect()
}

#[test]
fn another_time_is_proposed_from_the_invitation_in_the_mail() {
    // Through the item the account's server has put on the calendar.
    let (mut s, account, _, _) = session();
    serve(&mut s, account, "", vec![their_meeting("g-1", "Board", at(15, 13, 0))]);
    let message = mail(&mut s, account, "REQUEST", "uid-g-1", "");
    let told = run(&mut s, "calendar.invitation", json!({"message": message}));
    assert_eq!((&told["canPropose"], &told["answerable"], &told["proposed"]), (&json!(true), &json!(true), &Value::Null));
    let unasked = "Proposing another time writes to the meeting's organizer. Nothing was sent. Say that this is meant with \"send\": true.";
    assert_eq!(refused(&mut s, "calendar.propose", json!({"message": message, "start": "2026-10-16T13:00", "end": "2026-10-16T14:00"})), unasked);
    assert!(s.mailbox.event_changes.is_empty());
    let answer = run(&mut s, "calendar.propose", json!({"message": message, "start": "2026-10-16T13:00", "end": "2026-10-16T14:00", "send": true}));
    assert_eq!((&answer["onServer"], &answer["response"], &answer["sent"]), (&json!(true), &json!("tentative"), &json!(true)));
    let op = &ops(&s, account)[0];
    assert_eq!((op.kind, op.remote.as_str(), op.proposed.map(|time| time.start)), (EventChangeKind::Respond, "g-1", Some(at(16, 13, 0))));
    assert!(outbox(&s, account).is_empty(), "the server writes to the organizer, not the Outbox");

    // An organizer who takes no proposals says so in the invitation: nothing to press, and no way round.
    let (mut s, account, _, _) = session();
    serve(
        &mut s,
        account,
        "",
        vec![RemoteEvent {
            options: ResponseOptions { request_responses: true, allow_proposals: false },
            ..their_meeting("g-1", "Board", at(15, 13, 0))
        }],
    );
    let message = mail(&mut s, account, "REQUEST", "uid-g-1", "");
    assert_eq!(run(&mut s, "calendar.invitation", json!({"message": message}))["canPropose"], false);
    let why = refused(&mut s, "calendar.propose", json!({"message": message, "start": "2026-10-16T13:00", "end": "2026-10-16T14:00", "send": true}));
    assert_eq!(why, "The organizer of this meeting doesn't take proposals of another time.");

    // An account whose calendar is kept here: the proposal goes to the organizer as mail, from
    // the Outbox, as an iCalendar counter-proposal; and only on the owner's yes.
    let now = at(14, 16, 0);
    let mut s = Session::at(crate::sample::mailbox(now, 0), now, 0);
    let added = run(&mut s, "account.add", json!({"email": "ada@example.org", "name": "Ada", "incoming": {"host": "imap.example.org"}}));
    let account = added["id"].as_u64().unwrap();
    let message = mail(&mut s, account, "REQUEST", "uid-9", "");
    assert_eq!(run(&mut s, "calendar.invitation", json!({"message": message}))["canPropose"], true);
    let times = json!({"message": message, "start": "2026-10-16T13:00", "end": "2026-10-16T14:30"});
    assert_eq!(refused(&mut s, "calendar.propose", times.clone()), unasked);
    assert!(outbox(&s, account).is_empty());
    let mut yes = times.clone();
    yes["send"] = json!(true);
    yes["comment"] = json!("Friday suits me better.");
    let answer = run(&mut s, "calendar.propose", yes);
    assert_eq!((&answer["onServer"], &answer["sent"], &answer["response"]), (&json!(false), &json!(true), &json!("tentative")));
    let sent = outbox(&s, account);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        (sent[0].to.as_slice(), sent[0].subject.as_str()),
        ([Address::new("Bo Marsh", "bo@example.org")].as_slice(), "Another time proposed: Board")
    );
    assert!(
        sent[0]
            .body
            .text
            .starts_with("Friday suits me better.\n\nAda has tentatively accepted this meeting. Ada proposes another time: Friday, October 16, 2026"),
        "{}",
        sent[0].body.text
    );
    let ics = String::from_utf8(sent[0].attachments[0].data.to_vec()).unwrap();
    // (The time that is proposed is written as the moment it is: the clock here is UTC's.)
    for line in ["METHOD:COUNTER\r\n", "UID:uid-9\r\n", "DTSTART:20261016T130000Z\r\n", "DTEND:20261016T143000Z\r\n", "PARTSTAT=TENTATIVE"] {
        assert!(ics.contains(line), "{line} in {ics}");
    }
    // The meeting is on the calendar at the time its organizer has it, tentatively.
    let here = s.mailbox.events.values().find(|e| e.uid == "uid-9").unwrap();
    assert_eq!((here.start, here.show_as), (clock(15, 13, 0), mailcraft_model::ShowAs::Tentative));
    // An invitation that says the organizer takes no proposals.
    let closed = mail(&mut s, account, "REQUEST", "uid-10", "X-MICROSOFT-DISALLOW-COUNTER:TRUE\r\n");
    assert_eq!(run(&mut s, "calendar.invitation", json!({"message": closed}))["canPropose"], false);
    let why = refused(&mut s, "calendar.propose", json!({"message": closed, "start": "2026-10-16T13:00", "end": "2026-10-16T14:00", "send": true}));
    assert_eq!(why, "The organizer of this meeting doesn't take proposals of another time.");
    assert_eq!(outbox(&s, account).len(), 1);
}

#[test]
fn the_organizer_sees_what_was_proposed() {
    let (mut s, account, _, _) = session();
    let proposal = |name: &str, email: &str, day: u8| RemoteProposal { from: Address::new(name, email), start: at(day, 15, 0), end: at(day, 16, 0) };
    let planning = RemoteEvent {
        uid: "uid-m-1".into(),
        proposals: vec![proposal("Bo", "bo@example.org", 13), proposal("Cy", "cy@example.org", 14)],
        ..own_meeting("m-1", "Planning", at(12, 13, 0))
    };
    serve(&mut s, account, "", vec![planning]);
    let own = event_called(&s, "m-1");
    assert_eq!(own.proposals.len(), 2);
    assert_eq!(
        (own.proposals[0].from.email.as_str(), own.proposals[0].start, own.proposals[0].end),
        ("bo@example.org", clock(13, 15, 0), clock(13, 16, 0))
    );
    let listed = run(&mut s, "calendar.proposals", json!({"id": own.id}));
    assert_eq!(
        (&listed["event"], &listed["organizes"], &listed["title"], &listed["start"]),
        (&json!(own.id), &json!(true), &json!("Planning"), &json!("2026-10-12T13:00:00"))
    );
    assert_eq!(
        listed["proposals"],
        json!([
            {"name": "Bo", "email": "bo@example.org", "start": "2026-10-13T15:00:00", "end": "2026-10-13T16:00:00", "fromMessage": false},
            {"name": "Cy", "email": "cy@example.org", "start": "2026-10-14T15:00:00", "end": "2026-10-14T16:00:00", "fromMessage": false},
        ])
    );
    assert_eq!(run(&mut s, "calendar.get", json!({"id": own.id}))["proposals"].as_array().map(Vec::len), Some(2));

    // A message that proposes another time: what it proposes, the meeting it is about, and the
    // others that are known; what the calendar says too is listed once.
    let counter = "DTSTART:20261013T150000Z\r\nDTEND:20261013T160000Z\r\nATTENDEE;PARTSTAT=TENTATIVE;CN=Bo Marsh:mailto:bo@example.org\r\n";
    let ics = format!(
        "BEGIN:VCALENDAR\r\nMETHOD:COUNTER\r\nBEGIN:VEVENT\r\nUID:uid-m-1\r\nSUMMARY:Planning\r\n{counter}ORGANIZER:mailto:ada@example.org\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let message = Message {
        account,
        folder: inbox,
        from: Address::new("Bo Marsh", "bo@example.org"),
        subject: "Planning".into(),
        attachments: vec![Attachment::new("counter.ics", "text/calendar", ics.into_bytes())],
        ..Default::default()
    };
    let message = s.mailbox.add_message(message).unwrap();
    let told = run(&mut s, "calendar.invitation", json!({"message": message}));
    assert_eq!((&told["kind"], &told["event"], &told["mine"], &told["proposals"]), (&json!("counter"), &json!(own.id), &json!(true), &json!(2)));
    assert_eq!(told["proposed"], json!({"start": "2026-10-13T15:00:00", "end": "2026-10-13T16:00:00"}));
    let listed = run(&mut s, "calendar.proposals", json!({"message": message}));
    assert_eq!((&listed["event"], &listed["organizes"]), (&json!(own.id), &json!(true)));
    let people: Vec<(&str, bool)> =
        listed["proposals"].as_array().unwrap().iter().map(|one| (one["email"].as_str().unwrap(), one["fromMessage"].as_bool().unwrap())).collect();
    assert_eq!(people, [("bo@example.org", true), ("cy@example.org", false)]);
    // Any other message proposes nothing; reading changes nothing.
    let request = mail(&mut s, account, "REQUEST", "uid-x", "");
    let before = s.mailbox.clone();
    assert_eq!(refused(&mut s, "calendar.proposals", json!({"message": request})), "This message doesn't propose another time for a meeting.");
    run(&mut s, "calendar.proposals", json!({"message": message}));
    run(&mut s, "calendar.proposals", json!({"id": own.id}));
    assert!(s.mailbox == before, "reading changes nothing");
    assert_eq!(s.undo_label(), Some("Add Account"), "and is nothing to undo");

    // Accepting a proposal is changing the meeting to that time, which is an update like any
    // other: sent only on the owner's yes.
    let to = json!({"id": own.id, "start": "2026-10-13T15:00", "end": "2026-10-13T16:00"});
    assert_eq!(refused(&mut s, "calendar.edit", to.clone()), UPDATE);
    let mut yes = to;
    yes["send"] = json!(true);
    run(&mut s, "calendar.edit", yes);
    assert_eq!(ops(&s, account)[0].fields.time.map(|time| time.start), Some(at(13, 15, 0)));
}

#[test]
fn a_meeting_is_forwarded_only_on_the_owners_word() {
    let (mut s, account, _, _) = session();
    serve(
        &mut s,
        account,
        "",
        vec![their_meeting("g-1", "Board", at(15, 13, 0)), own_meeting("m-1", "Planning", at(12, 13, 0)), remote("e-1", "Eye test", at(13, 9, 0))],
    );
    let (board, planning) = (event_called(&s, "g-1").id, event_called(&s, "m-1").id);
    let snapshot = s.mailbox.clone();
    // It writes to people: nothing without the yes.
    for (params, why) in [
        (
            json!({"id": board, "to": "dana@example.org"}),
            "Forwarding writes to 1 person. Nothing was sent. Say that this is meant with \"send\": true.",
        ),
        (
            json!({"id": board, "to": "dana@example.org; Eli <eli@example.org>", "send": false}),
            "Forwarding writes to 2 people. Nothing was sent. Say that this is meant with \"send\": true.",
        ),
        (
            json!({"id": board, "to": ["dana@example.org", "eli@example.org"], "send": 1}),
            "Forwarding writes to 2 people. Nothing was sent. Say that this is meant with \"send\": true.",
        ),
        (json!({"id": board, "to": "dana@example.org; not an address", "send": true}), "\"not an address\" isn't an e-mail address"),
    ] {
        assert_eq!(refused(&mut s, "calendar.forward", params.clone()), why, "{params}");
    }
    for params in [json!({"id": board, "send": true}), json!({"id": board, "to": " ; ", "send": true}), json!({"id": board, "to": 7, "send": true})] {
        assert!(matches!(s.run("calendar.forward", &params), Err(CmdError::Params(_))), "{params}");
    }
    assert!(s.mailbox == snapshot);

    // With it: noted for the server, with who it goes to and the note; nothing changes here.
    let events = s.mailbox.events.clone();
    let done = run(
        &mut s,
        "calendar.forward",
        json!({"id": board, "to": "Dana Swope <dana@example.org>; eli@example.org; DANA@example.org", "comment": " Hope you can make it. ", "send": true}),
    );
    assert_eq!(done, json!({"id": board, "to": 2, "onServer": true}));
    assert!(s.mailbox.events == events);
    let told = ops(&s, account);
    assert_eq!(told.len(), 1);
    assert_eq!(
        (told[0].kind, told[0].remote.as_str(), told[0].send, told[0].comment.as_str()),
        (EventChangeKind::Forward, "g-1", true, "Hope you can make it.")
    );
    assert_eq!(told[0].to, [Address::new("Dana Swope", "dana@example.org"), Address::new("", "eli@example.org")]);
    assert!(told[0].fields.is_empty() && !told[0].token.is_empty());
    // It waits beside a change to the same meeting, and beside another forward of it.
    run(&mut s, "calendar.reminder", json!({"id": board, "minutes": 5}));
    run(&mut s, "calendar.forward", json!({"id": board, "to": "fay@example.org", "send": true}));
    let kinds: Vec<EventChangeKind> = ops(&s, account).iter().map(|op| op.kind).collect();
    assert_eq!(kinds, [EventChangeKind::Forward, EventChangeKind::Update, EventChangeKind::Forward]);
    // A download meanwhile goes on saying what the meeting is.
    let later = RemoteEvent { title: "Board (moved)".into(), change_key: "ck-2".into(), ..their_meeting("g-1", "Board", at(15, 14, 0)) };
    serve(&mut s, account, "", vec![later.clone(), own_meeting("m-1", "Planning", at(12, 13, 0)), remote("e-1", "Eye test", at(13, 9, 0))]);
    assert_eq!(ops(&s, account).len(), 3);
    // A mailbox file that says a meeting is to be forwarded, and not that the owner said so: it doesn't go.
    let noted = s.mailbox.event_changes.clone();
    for change in s.mailbox.event_changes.iter_mut().filter(|change| change.kind == EventChangeKind::Forward) {
        change.send = false;
    }
    assert_eq!(ops(&s, account).iter().map(|op| op.kind).collect::<Vec<_>>(), [EventChangeKind::Update]);
    // Nor to nobody, nor a meeting that isn't here.
    s.mailbox.event_changes = noted.clone();
    s.mailbox.event_changes[0].to.clear();
    s.mailbox.event_changes[2].remote = "somebody-elses".into();
    assert_eq!(ops(&s, account).iter().map(|op| op.kind).collect::<Vec<_>>(), [EventChangeKind::Update]);
    s.mailbox.event_changes = noted;

    // The server did it: nothing waits, nothing is said.
    let applied = push(&mut s, account, EventResult::Done, "");
    assert!(applied.messages.is_empty() && s.mailbox.event_changes.is_empty(), "{applied:?}");
    // It couldn't be reached: the forward waits, and is tried again.
    run(&mut s, "calendar.forward", json!({"id": planning, "to": "dana@example.org", "send": true}));
    let applied = push(&mut s, account, EventResult::Failed, "no connection");
    assert_eq!((applied.failed, s.mailbox.event_changes.len()), (1, 1));
    assert!(ops(&s, account).is_empty(), "not before its time");
    s.mailbox.event_changes[0].retry_at = 0;
    // It won't: that is said, and nothing is left to try.
    let applied = push(&mut s, account, EventResult::Refused, "Microsoft Graph answered with status 400");
    assert_eq!(applied.messages, ["The server wouldn't forward \"Planning\" (Microsoft Graph answered with status 400). Nobody was written to."]);
    assert!(s.mailbox.event_changes.is_empty());
    run(&mut s, "calendar.forward", json!({"id": planning, "to": "dana@example.org", "send": true}));
    let applied = push(&mut s, account, EventResult::Gone, "");
    assert_eq!(applied.messages, ["\"Planning\" is no longer on the server, so it wasn't forwarded."]);
    // Undone before the server heard: nobody is written to.
    run(&mut s, "calendar.forward", json!({"id": planning, "to": "dana@example.org", "send": true}));
    assert_eq!(s.undo_label(), Some("Forward"));
    assert!(s.undo() && s.mailbox.event_changes.is_empty());

    // One that hasn't reached the server yet can't be forwarded by it.
    let fresh = run(&mut s, "calendar.new", json!({"title": "Fresh", "start": "2026-10-20T09:00"}))["id"].clone();
    let why = refused(&mut s, "calendar.forward", json!({"id": fresh, "to": "dana@example.org", "send": true}));
    assert_eq!(why, "This item hasn't reached the server of Ada yet. Forward it once it is there (in a moment).");
}

#[test]
fn an_item_kept_on_this_computer_is_forwarded_as_mail_with_the_item_attached() {
    let now = at(14, 16, 0);
    let mut s = Session::at(crate::sample::mailbox(now, 0), now, 0);
    let review = s.mailbox.events.values().find(|e| e.title.starts_with("Design review")).map(|e| e.as_ref().clone()).unwrap();
    assert!(review.is_meeting() && review.origin.is_local());
    // A mailbox with no account that has a mail server to send through says so.
    let nowhere = "No account here has a mail server to send this through.";
    assert_eq!(refused(&mut s, "calendar.forward", json!({"id": review.id, "to": "dana@example.org", "send": true})), nowhere);
    let added = run(&mut s, "account.add", json!({"email": "ada@example.org", "name": "Ada", "incoming": {"host": "imap.example.org"}}));
    let account = added["id"].as_u64().unwrap();
    let before = outbox(&s, account).len();
    let unasked = "Forwarding writes to 1 person. Nothing was sent. Say that this is meant with \"send\": true.";
    assert_eq!(refused(&mut s, "calendar.forward", json!({"id": review.id, "to": "dana@example.org"})), unasked);
    assert_eq!(outbox(&s, account).len(), before);
    let done = run(&mut s, "calendar.forward", json!({"id": review.id, "to": "Dana <dana@example.org>", "comment": "For you.", "send": true}));
    assert_eq!(done, json!({"id": review.id, "to": 1, "onServer": false}));
    let sent = outbox(&s, account);
    assert_eq!(sent.len(), before + 1);
    let message = sent.iter().find(|m| m.subject.starts_with("FW: ")).unwrap();
    assert_eq!(
        (message.to.as_slice(), message.subject.as_str()),
        ([Address::new("Dana", "dana@example.org")].as_slice(), format!("FW: {}", review.title).as_str())
    );
    assert!(message.body.text.starts_with("For you.\n\n") && message.body.text.contains(&review.title), "{}", message.body.text);
    // The item itself, for them to keep: what, when and where; not who else is invited, nor what
    // the owner made of it for their own use.
    let ics = String::from_utf8(message.attachments[0].data.to_vec()).unwrap();
    assert!(ics.contains("METHOD:PUBLISH\r\n") && ics.contains("BEGIN:VEVENT\r\n") && ics.contains("SUMMARY:Design review"), "{ics}");
    assert!(!ics.contains("ATTENDEE") && !ics.contains("VALARM") && !ics.contains("CATEGORIES"), "{ics}");
    assert_eq!(s.mailbox.event(review.id), Some(&review), "nothing of the item changes");
    assert!(s.mailbox.event_changes.is_empty());
    // One step to take back, before it is sent.
    assert!(s.undo() && outbox(&s, account).len() == before);
}

#[test]
fn an_appointment_is_moved_to_another_calendar_of_its_account() {
    let (mut s, account, main, side) = session();
    serve(&mut s, account, "", vec![remote("e-1", "Eye test", at(13, 9, 0)), remote("e-2", "Gym", at(14, 18, 0))]);
    let dentist = event_called(&s, "e-1");
    assert_eq!(dentist.calendar, main);
    // Into the other calendar, an hour later while it is at it (a drop there): one step here.
    let moved = run(&mut s, "calendar.edit", json!({"id": dentist.id, "calendar": side, "start": "2026-10-13T10:00"}));
    assert_eq!(moved, json!({"id": dentist.id, "moved": true}));
    let here = s.mailbox.event(dentist.id).unwrap().clone();
    assert_eq!((here.calendar, here.start, here.origin.account, here.origin.id.as_str()), (side, clock(13, 10, 0), account, ""));
    assert_eq!(s.undo_label(), Some("Edit Event"));
    // The server has no way of moving an event: it is made anew in the other calendar first.
    let upload = event_upload(&s, account).unwrap();
    assert_eq!(upload.ops.len(), 1);
    let op = &upload.ops[0];
    assert_eq!(
        (op.kind, op.calendar.as_str(), op.event, op.fields.title.as_deref()),
        (EventChangeKind::Create, "c-side", dentist.id, Some("Eye test"))
    );
    assert_eq!(op.fields.time.map(|time| time.start), Some(at(13, 10, 0)));
    // Until that is done the original, which the server still has, doesn't come back as a second one.
    serve(&mut s, account, "", vec![remote("e-1", "Eye test", at(13, 9, 0)), remote("e-2", "Gym", at(14, 18, 0))]);
    assert_eq!(s.mailbox.events.values().filter(|e| e.title == "Eye test").count(), 1);
    // Made, the server says: that is yet to be seen there, and the original stays until it is.
    let copy = format!("made-{}", dentist.id);
    let applied = apply_event_outcomes(&mut s, &said(&upload, EventResult::Done, "")).unwrap();
    assert!(applied.messages.is_empty());
    assert_eq!((applied.unverified, applied.waiting), (1, 1));
    assert_eq!(s.mailbox.event(dentist.id).map(|e| (e.origin.id.as_str(), e.calendar)), Some((copy.as_str(), side)));
    assert!(ops(&s, account).is_empty(), "the original is not deleted on the server's word alone");
    let waiting = events_waiting(&s, account);
    assert_eq!((waiting.due, waiting.total, waiting.next.is_some()), (0, 1, true));
    // A look at the other calendar that was answered from before the copy decides nothing; nor
    // does the original, which the server still has, come back as a second one.
    serve(&mut s, account, "c-side", Vec::new());
    serve(&mut s, account, "", vec![remote("e-1", "Eye test", at(13, 9, 0)), remote("e-2", "Gym", at(14, 18, 0))]);
    assert!(ops(&s, account).is_empty());
    assert_eq!(s.mailbox.events.values().filter(|e| e.title == "Eye test").map(|e| e.calendar).collect::<Vec<_>>(), [side]);
    // The copy is there: only now is the original to go, out of the calendar it is in.
    let theirs = vec![RemoteEvent { change_key: "ck-new".into(), ..remote(&copy, "Eye test", at(13, 10, 0)) }];
    let merged = apply_events(&mut s, &october(account, "c-side", theirs.clone())).unwrap();
    assert_eq!((merged["verified"].as_u64(), merged["putBack"].as_u64()), (Some(1), Some(0)));
    let upload = event_upload(&s, account).unwrap();
    assert_eq!(
        upload.ops.iter().map(|op| (op.kind, op.remote.as_str(), op.calendar.as_str(), op.cancel)).collect::<Vec<_>>(),
        [(EventChangeKind::Delete, "e-1", "", false)]
    );
    serve(&mut s, account, "", vec![remote("e-1", "Eye test", at(13, 9, 0)), remote("e-2", "Gym", at(14, 18, 0))]);
    assert_eq!(s.mailbox.events.values().filter(|e| e.title == "Eye test").count(), 1, "still one, while the original waits to go");
    apply_event_outcomes(&mut s, &said(&upload, EventResult::Done, "")).unwrap();
    assert!(s.mailbox.event_changes.is_empty());
    // A main calendar that was answered from before the deletion doesn't bring the original
    // back; and what the server has then is what is here: one of them, in the other calendar.
    serve(&mut s, account, "", vec![remote("e-1", "Eye test", at(13, 9, 0)), remote("e-2", "Gym", at(14, 18, 0))]);
    assert_eq!(s.mailbox.events.values().filter(|e| e.title == "Eye test").count(), 1);
    let merged = apply_events(&mut s, &october(account, "", vec![remote("e-2", "Gym", at(14, 18, 0))])).unwrap();
    assert_eq!((merged["verified"].as_u64(), merged["putBack"].as_u64(), merged["messages"].as_array().map(Vec::len)), (Some(1), Some(0), Some(0)));
    serve(&mut s, account, "c-side", theirs);
    let all: Vec<(Id, Id)> = s.mailbox.events.values().filter(|e| e.title == "Eye test").map(|e| (e.id, e.calendar)).collect();
    assert_eq!(all, [(dentist.id, side)]);
    assert!(s.mailbox.event_checks.is_empty() && s.mailbox.clone().repair().is_empty());

    // Taken back before the server heard of it: nothing is told, and it is as it was.
    let gym = event_called(&s, "e-2");
    run(&mut s, "calendar.edit", json!({"id": gym.id, "calendar": side}));
    assert!(s.undo());
    assert_eq!(s.mailbox.event(gym.id), Some(&gym));
    assert!(s.mailbox.event_changes.is_empty());
    // Taken back while it was on its way: the copy the server made goes again, and the original stays.
    run(&mut s, "calendar.edit", json!({"id": gym.id, "calendar": side}));
    let upload = event_upload(&s, account).unwrap();
    assert!(s.undo());
    apply_event_outcomes(&mut s, &said(&upload, EventResult::Done, "")).unwrap();
    assert_eq!(
        ops(&s, account).iter().map(|op| (op.kind, op.remote.clone())).collect::<Vec<_>>(),
        [(EventChangeKind::Delete, format!("made-{}", gym.id))]
    );
    assert_eq!(s.mailbox.event(gym.id).map(|e| (e.calendar, e.origin.id.as_str())), Some((main, "e-2")));
    push(&mut s, account, EventResult::Done, "");

    // Deleted while it was on its way to the other calendar: what goes is the original.
    run(&mut s, "calendar.edit", json!({"id": gym.id, "calendar": side}));
    run(&mut s, "calendar.delete", json!({"id": gym.id}));
    assert_eq!(ops(&s, account).iter().map(|op| (op.kind, op.remote.clone())).collect::<Vec<_>>(), [(EventChangeKind::Delete, "e-2".to_string())]);
    serve(&mut s, account, "", vec![remote("e-2", "Gym", at(14, 18, 0))]);
    assert!(s.mailbox.events.values().all(|e| e.title != "Gym"), "it stays deleted");
}

#[test]
fn a_move_the_server_refuses_is_put_back_and_says_so() {
    let (mut s, account, main, side) = session();
    serve(&mut s, account, "", vec![remote("e-1", "Eye test", at(13, 9, 0))]);
    let dentist = event_called(&s, "e-1");
    // The copy is refused: back where it was, as it was.
    run(&mut s, "calendar.edit", json!({"id": dentist.id, "calendar": side, "start": "2026-10-13T10:00"}));
    let applied = push(&mut s, account, EventResult::Refused, "Microsoft Graph answered with status 400");
    assert_eq!(
        applied.messages,
        [
            "The server wouldn't take \"Eye test\" into the other calendar (Microsoft Graph answered with status 400), so it wasn't moved: it is back where it was."
        ]
    );
    assert_eq!(s.mailbox.event(dentist.id), Some(&dentist));
    assert!(s.mailbox.event_changes.is_empty());
    // It can't be reached: the move waits as it is, and nothing is lost or doubled meanwhile.
    run(&mut s, "calendar.edit", json!({"id": dentist.id, "calendar": side}));
    let applied = push(&mut s, account, EventResult::Failed, "no connection");
    assert_eq!((applied.failed, s.mailbox.event(dentist.id).map(|e| e.calendar)), (1, Some(side)));
    serve(&mut s, account, "", vec![remote("e-1", "Eye test", at(13, 9, 0))]);
    assert_eq!(s.mailbox.events.values().filter(|e| e.title == "Eye test").count(), 1);
    s.mailbox.event_changes[0].retry_at = 0;
    // The copy is made and seen there, and then the original won't go: the copy is taken out
    // again, here and there, and the appointment is back where it was.
    let copy = format!("made-{}", dentist.id);
    push(&mut s, account, EventResult::Done, "");
    serve(&mut s, account, "c-side", vec![RemoteEvent { change_key: "ck-new".into(), ..remote(&copy, "Eye test", at(13, 9, 0)) }]);
    assert_eq!(ops(&s, account).iter().map(|op| (op.kind, op.remote.clone())).collect::<Vec<_>>(), [(EventChangeKind::Delete, "e-1".to_string())]);
    let applied = push(&mut s, account, EventResult::Refused, "Microsoft Graph answered with status 409");
    assert_eq!(
        applied.messages,
        [
            "The server wouldn't take \"Eye test\" out of the calendar it was in (Microsoft Graph answered with status 409), so it wasn't moved: it is back there, and the copy in the other calendar was removed."
        ]
    );
    assert_eq!(s.mailbox.event(dentist.id), Some(&dentist));
    assert_eq!(s.mailbox.events.values().filter(|e| e.title == "Eye test").count(), 1);
    // (The copy is asked for in the calendar it is in, not in the one the appointment is back in.)
    assert_eq!(
        ops(&s, account).iter().map(|op| (op.kind, op.remote.clone(), op.calendar.clone(), op.cancel)).collect::<Vec<_>>(),
        [(EventChangeKind::Delete, copy.clone(), "c-side".to_string(), false)]
    );
    push(&mut s, account, EventResult::Done, "");
    assert!(s.mailbox.event_changes.is_empty());
    assert_eq!(s.mailbox.event(dentist.id).map(|e| e.calendar), Some(main));
    // The copy is gone there, the original is still there: nothing is left to look for.
    serve(&mut s, account, "c-side", Vec::new());
    serve(&mut s, account, "", vec![remote("e-1", "Eye test", at(13, 9, 0))]);
    assert_eq!(s.mailbox.event(dentist.id), Some(&dentist));
    assert!(s.mailbox.event_checks.is_empty() && s.mailbox.clone().repair().is_empty());
}

#[test]
fn the_original_of_a_move_goes_only_when_the_copy_is_seen_on_the_server() {
    let (mut s, account, main, side) = session();
    let theirs = || vec![remote("e-1", "Eye test", at(13, 9, 0)), remote("e-2", "Gym", at(14, 18, 0))];
    serve(&mut s, account, "", theirs());
    let (dentist, gym) = (event_called(&s, "e-1"), event_called(&s, "e-2"));
    // The server says it made the copy, and the other calendar never has it: after three looks
    // the move is taken back. The original was not touched, so nothing is lost: the appointment
    // is where it was, and the owner is told.
    run(&mut s, "calendar.edit", json!({"id": dentist.id, "calendar": side}));
    push(&mut s, account, EventResult::Done, "");
    for look in 1..=2 {
        let merged = apply_events(&mut s, &october(account, "c-side", Vec::new())).unwrap();
        assert_eq!((merged["putBack"].as_u64(), merged["unverified"].as_u64()), (Some(0), Some(1)), "look {look}");
        assert!(ops(&s, account).is_empty(), "look {look}");
        serve(&mut s, account, "", theirs());
        assert_eq!(s.mailbox.event(dentist.id).map(|e| e.calendar), Some(side), "look {look}");
    }
    let merged = apply_events(&mut s, &october(account, "c-side", Vec::new())).unwrap();
    assert_eq!((merged["putBack"].as_u64(), merged["unverified"].as_u64(), merged["removed"].as_u64()), (Some(1), Some(0), Some(0)));
    assert_eq!(
        merged["messages"],
        json!([
            "\"Eye test\" was moved to another calendar here and the server said it took it there, but that calendar doesn't have it. It wasn't moved: it is back where it was."
        ])
    );
    assert_eq!(s.mailbox.event(dentist.id), Some(&dentist));
    assert!(s.mailbox.event_changes.is_empty() && s.mailbox.event_checks.is_empty() && ops(&s, account).is_empty());
    serve(&mut s, account, "", theirs());
    assert_eq!(s.mailbox.events.values().filter(|e| e.title == "Eye test").map(|e| (e.id, e.calendar)).collect::<Vec<_>>(), [(dentist.id, main)]);
    assert!(s.mailbox.clone().repair().is_empty());

    // A copy that no download can show (its calendar isn't asked for): the original is not
    // kept waiting for ever. After two minutes it goes on the server's word, as any change
    // that can't be looked for is taken on it.
    run(&mut s, "calendar.edit", json!({"id": gym.id, "calendar": side}));
    assert!(crate::cmd::remote::unverified_calendars(&s, account).is_empty());
    push(&mut s, account, EventResult::Done, "");
    // (A host asks for the calendar the copy is in while that is so, ticked or not.)
    assert_eq!(crate::cmd::remote::unverified_calendars(&s, account), [side]);
    serve(&mut s, account, "", theirs());
    assert!(ops(&s, account).is_empty());
    let next = events_waiting(&s, account).next.unwrap();
    assert!(next > s.now() && next <= s.now() + 121, "{next}");
    s.clock.now = Some(next);
    assert_eq!(events_waiting(&s, account).due, 1);
    assert_eq!(
        ops(&s, account).iter().map(|op| (op.kind, op.remote.as_str(), op.calendar.as_str())).collect::<Vec<_>>(),
        [(EventChangeKind::Delete, "e-2", "")]
    );
}

#[test]
fn what_would_not_arrive_whole_is_not_moved() {
    let (mut s, account, _, side) = session();
    let local = s.mailbox.calendars.iter().find(|c| c.origin.is_local()).map(|c| c.id).unwrap();
    let weekly = RemoteEvent {
        kind: mailcraft_model::remote::RemoteEventKind::SeriesMaster,
        recurrence: Some(Recurrence::every(Freq::Weekly)),
        series_day: Date::new(2026, 10, 5),
        ..remote("s-1", "Gym", at(5, 18, 0))
    };
    let changed = RemoteEvent {
        kind: mailcraft_model::remote::RemoteEventKind::Exception,
        series: "s-1".into(),
        original: Some(at(12, 18, 0)),
        ..remote("x-1", "Gym (late)", at(12, 19, 0))
    };
    serve(
        &mut s,
        account,
        "",
        vec![
            own_meeting("m-1", "Planning", at(12, 13, 0)),
            their_meeting("g-1", "Board", at(15, 13, 0)),
            weekly,
            changed,
            RemoteEvent { attachments: true, ..remote("e-3", "Contract", at(16, 9, 0)) },
            remote("e-1", "Eye test", at(13, 9, 0)),
        ],
    );
    let id = |name: &str| event_called(&s, name).id;
    let (planning, board, gym, late, contract, dentist) = (id("m-1"), id("g-1"), id("s-1"), id("x-1"), id("e-3"), id("e-1"));
    let snapshot = s.mailbox.clone();
    for (event, into, why) in [
        (
            planning,
            side,
            "A meeting can't be moved to another calendar: the account's server has no way of moving one, and making it again there would send everyone invited a cancellation and a new invitation.",
        ),
        (board, side, "Somebody else organizes this meeting: it stays in the calendar its invitation came to."),
        (
            gym,
            side,
            "A series that the account's server keeps can't be moved to another calendar from here: the server has no way of moving one, and days of it that were changed on their own could be lost in making it again.",
        ),
        (late, side, "One occurrence of a series can't be in another calendar than the rest of the series."),
        (
            contract,
            side,
            "This item has files attached, which only the server keeps and which would be lost: it can't be moved to another calendar from here.",
        ),
        (
            dentist,
            local,
            "This event is kept by the server of Ada. It can be moved to another calendar of that account, not out of it: taking it out would delete it there.",
        ),
    ] {
        assert_eq!(refused(&mut s, "calendar.edit", json!({"id": event, "calendar": into})), why);
        assert!(s.mailbox == snapshot);
    }
    // The owner's yes to mail doesn't make a meeting movable.
    assert!(refused(&mut s, "calendar.edit", json!({"id": planning, "calendar": side, "send": true})).starts_with("A meeting can't be moved"));
    // Into a calendar the account may only read; and one that is still on its way.
    s.mailbox.calendars.iter_mut().find(|c| c.id == side).unwrap().rights = mailcraft_model::Rights::Read;
    assert_eq!(
        refused(&mut s, "calendar.edit", json!({"id": dentist, "calendar": side})),
        "The calendar \"Side\" can only be read: its owner hasn't let this account change it."
    );
    s.mailbox.calendars.iter_mut().find(|c| c.id == side).unwrap().rights = mailcraft_model::Rights::Edit;
    run(&mut s, "calendar.edit", json!({"id": dentist, "title": "Eye test, again"}));
    let waiting = "This item is still on its way to the account's server. Move it once it is there (in a moment).";
    assert_eq!(refused(&mut s, "calendar.edit", json!({"id": dentist, "calendar": side})), waiting);
    let fresh = run(&mut s, "calendar.new", json!({"title": "Fresh", "start": "2026-10-20T09:00"}))["id"].clone();
    assert_eq!(refused(&mut s, "calendar.edit", json!({"id": fresh, "calendar": side})), waiting);
    // A copy is another matter: a plain appointment of its own in the other calendar.
    let copy = run(&mut s, "calendar.copy", json!({"id": planning, "start": "2026-10-12T13:00", "calendar": side}));
    let copy = s.mailbox.event(copy["id"].as_u64().unwrap()).unwrap();
    assert!(copy.calendar == side && copy.attendees.is_empty() && copy.origin.id.is_empty());
}

#[test]
fn a_hostile_file_cannot_make_the_server_write_to_anybody() {
    // Whatever a mailbox file notes for the server, mail to other people goes only with the
    // owner's yes beside it; and what is noted is cut to what could be meant.
    let (mut s, account, _, _) = session();
    serve(&mut s, account, "", vec![their_meeting("g-1", "Board", at(15, 13, 0)), own_meeting("m-1", "Planning", at(12, 13, 0))]);
    let (board, planning) = (event_called(&s, "g-1"), event_called(&s, "m-1"));
    let note = |kind: EventChangeKind, event: &Event| mailcraft_model::remote::EventChange {
        account,
        kind,
        event: event.id,
        remote: event.origin.id.clone(),
        token: "0f8fad5b-d9cb-469f-a165-70867728950e".into(),
        base: Some(Box::new(event.clone())),
        comment: "x".into(),
        ..Default::default()
    };
    let time = mailcraft_model::remote::EventTime { start: at(16, 13, 0), end: at(16, 14, 0), all_day: false };
    let people = vec![Address::new("Eve", "eve@example.org")];
    s.mailbox.event_changes = vec![
        mailcraft_model::remote::EventChange { to: people.clone(), ..note(EventChangeKind::Forward, &planning) },
        mailcraft_model::remote::EventChange { response: Response::Tentative, proposed: Some(time), ..note(EventChangeKind::Respond, &board) },
    ];
    assert!(ops(&s, account).is_empty());
    assert_eq!(events_waiting(&s, account).total, 0);
    // With the yes they would go: it is the yes that is missing, nothing else.
    for change in &mut s.mailbox.event_changes {
        change.send = true;
    }
    assert_eq!(ops(&s, account).len(), 2);
    // An answer that isn't one doesn't carry a proposal either.
    s.mailbox.event_changes[1].response = Response::Accepted;
    assert_eq!(ops(&s, account).iter().find(|op| op.kind == EventChangeKind::Respond).and_then(|op| op.proposed), None);
    // Answers that were never asked for change nothing, whatever they say.
    let before = s.mailbox.clone();
    let odd = |kind: EventChangeKind| EventOutcome {
        op: EventOp {
            kind,
            event: board.id,
            remote: "g-1".into(),
            token: "no-such-mark".into(),
            to: people.clone(),
            send: true,
            ..Default::default()
        },
        result: EventResult::Refused,
        online: Some(false),
        join_url: "javascript:alert(1)".into(),
        error: "x".repeat(10_000),
        ..Default::default()
    };
    let uploaded = EventsUploaded {
        account,
        email: "ada@example.org".into(),
        outcomes: vec![odd(EventChangeKind::Forward), odd(EventChangeKind::Create)],
        ..Default::default()
    };
    apply_event_outcomes(&mut s, &uploaded).unwrap();
    assert!(s.mailbox.events == before.events);
    // Nor do such notes lie about for ever: without the yes they are given up, with a word each
    // for the owner, and nothing of the meetings is changed.
    for change in &mut s.mailbox.event_changes {
        change.send = false;
    }
    s.mailbox.event_changes[1].response = Response::Tentative;
    let dropped = crate::cmd::remote::settle_changes(&mut s);
    assert_eq!(
        dropped.iter().map(|one| one.message.as_str()).collect::<Vec<_>>(),
        [
            "\"Planning\" was not forwarded: that writes to other people, and nobody said to send it. Nobody was written to.",
            "The other time proposed for \"Board\" was not sent: that writes to its organizer, and nobody said to send it. The next Send/Receive shows the meeting as the server has it.",
        ]
    );
    assert!(s.mailbox.event_changes.is_empty() && s.mailbox.events == before.events);
}

#[test]
fn a_deletion_answered_not_found_is_looked_for_like_one_that_was_taken() {
    let (mut s, account, main, _) = session();
    let gym = || remote("e-2", "Gym", at(14, 18, 0));
    serve(&mut s, account, "", vec![remote("e-1", "Eye test", at(13, 9, 0)), gym()]);
    let look = |s: &mut Session, events: Vec<RemoteEvent>| apply_events(s, &october(account, "", events)).unwrap();
    // Deleted in Outlook too, meanwhile: the server has no such item, and what comes down
    // agrees. It is gone, and nothing is said.
    let dentist = event_called(&s, "e-1").id;
    run(&mut s, "calendar.delete", json!({"id": dentist}));
    let applied = push(&mut s, account, EventResult::Gone, "");
    assert_eq!((applied.done, applied.unverified, applied.messages.len()), (1, 1, 0));
    assert!(s.mailbox.event_checks[0].not_found);
    let merged = look(&mut s, vec![gym()]);
    assert_eq!((merged["verified"].as_u64(), merged["putBack"].as_u64(), merged["messages"].as_array().map(Vec::len)), (Some(1), Some(0), Some(0)));
    assert!(s.mailbox.event_checks.is_empty() && s.mailbox.events.values().all(|e| e.title != "Eye test"));

    // "No such item" for one the server has all the same (it was asked for at an address where
    // it isn't): it is not taken for gone. After three looks it is back, with a line.
    let id = event_called(&s, "e-2").id;
    run(&mut s, "calendar.delete", json!({"id": id}));
    push(&mut s, account, EventResult::Gone, "");
    for n in 1..=2 {
        let merged = look(&mut s, vec![gym()]);
        assert_eq!((merged["unverified"].as_u64(), merged["added"].as_u64()), (Some(1), Some(0)), "look {n}");
        assert!(s.mailbox.events.values().all(|e| e.title != "Gym"), "look {n}");
    }
    let merged = look(&mut s, vec![gym()]);
    assert_eq!((merged["putBack"].as_u64(), merged["added"].as_u64(), merged["unverified"].as_u64()), (Some(1), Some(1), Some(0)));
    assert_eq!(
        merged["messages"],
        json!(["\"Gym\" was deleted here and the server answered that it has no such item, but it still has it. It is back in the calendar."])
    );
    assert_eq!(event_called(&s, "e-2").calendar, main);
    // It is said once: the next look finds it as it is, and has nothing to add.
    let merged = look(&mut s, vec![gym()]);
    assert_eq!((merged["messages"].as_array().map(Vec::len), merged["unchanged"].as_u64()), (Some(0), Some(1)));
    assert!(s.mailbox.event_checks.is_empty() && s.mailbox.event_changes.is_empty() && s.mailbox.clone().repair().is_empty());
}

#[test]
fn a_move_is_not_done_on_a_not_found_for_the_original() {
    let (mut s, account, main, side) = session();
    let gym = || remote("e-2", "Gym", at(14, 18, 0));
    serve(&mut s, account, "", vec![gym()]);
    let before = event_called(&s, "e-2");
    let copy = format!("made-{}", before.id);
    // Into the other calendar: the copy is made and seen there, and then the server answers
    // "no such item" when the original is to go.
    run(&mut s, "calendar.edit", json!({"id": before.id, "calendar": side}));
    push(&mut s, account, EventResult::Done, "");
    serve(&mut s, account, "c-side", vec![RemoteEvent { change_key: "ck-new".into(), ..remote(&copy, "Gym", at(14, 18, 0)) }]);
    assert_eq!(ops(&s, account).iter().map(|op| (op.kind, op.remote.as_str())).collect::<Vec<_>>(), [(EventChangeKind::Delete, "e-2")]);
    let applied = push(&mut s, account, EventResult::Gone, "");
    assert_eq!((applied.unverified, applied.messages.len()), (1, 0));
    // The calendar it was in still has it. That is no move: after three looks the copy goes
    // again, here and (next) on the server, and the appointment is where the server has it.
    for n in 1..=2 {
        let merged = apply_events(&mut s, &october(account, "", vec![gym()])).unwrap();
        assert_eq!((merged["unverified"].as_u64(), merged["added"].as_u64()), (Some(1), Some(0)), "look {n}");
        assert_eq!(s.mailbox.events.values().filter(|e| e.title == "Gym").map(|e| e.calendar).collect::<Vec<_>>(), [side], "look {n}");
    }
    let merged = apply_events(&mut s, &october(account, "", vec![gym()])).unwrap();
    assert_eq!(
        merged["messages"],
        json!([
            "\"Gym\" was moved to another calendar here, and when it was to be taken out of the one it was in the server answered that it has no such item, but it still has it there. It wasn't moved: it is back there, and the copy in the other calendar was removed."
        ])
    );
    let here: Vec<(Id, String)> = s.mailbox.events.values().filter(|e| e.title == "Gym").map(|e| (e.calendar, e.origin.id.clone())).collect();
    assert_eq!(here, [(main, "e-2".to_string())]);
    assert_eq!(
        ops(&s, account).iter().map(|op| (op.kind, op.remote.clone(), op.calendar.clone())).collect::<Vec<_>>(),
        [(EventChangeKind::Delete, copy, "c-side".to_string())]
    );
    assert!(s.mailbox.event_checks.is_empty());
}

#[test]
fn what_is_undone_while_it_is_being_made_is_deleted_where_it_was_made() {
    let (mut s, account, _, side) = session();
    let made = run(&mut s, "calendar.new", json!({"title": "Fresh", "start": "2026-10-20T09:00", "calendar": side}))["id"].as_u64().unwrap();
    let upload = event_upload(&s, account).unwrap();
    assert_eq!((upload.ops[0].kind, upload.ops[0].calendar.as_str()), (EventChangeKind::Create, "c-side"));
    // Taken back while it is on its way; then the server says it made it.
    assert!(s.undo());
    apply_event_outcomes(&mut s, &said(&upload, EventResult::Done, "")).unwrap();
    assert!(s.mailbox.event(made).is_none());
    // It is to be deleted there: through the calendar it was made in.
    let name = format!("made-{made}");
    assert_eq!(
        ops(&s, account).iter().map(|op| (op.kind, op.remote.clone(), op.calendar.clone())).collect::<Vec<_>>(),
        [(EventChangeKind::Delete, name.clone(), "c-side".to_string())]
    );
    // And it is looked for like any deletion: what another calendar has says nothing of it,
    // and it does not come back while that calendar still shows it.
    push(&mut s, account, EventResult::Done, "");
    serve(&mut s, account, "", Vec::new());
    serve(&mut s, account, "c-side", vec![remote(&name, "Fresh", at(20, 9, 0))]);
    assert_eq!((s.mailbox.event_checks.len(), s.mailbox.events.values().filter(|e| e.title == "Fresh").count()), (1, 0));
    serve(&mut s, account, "c-side", Vec::new());
    assert!(s.mailbox.event_checks.is_empty() && s.mailbox.event_changes.is_empty());
}

#[test]
fn an_option_the_server_will_not_change_does_not_undo_the_rest_of_a_save() {
    let (mut s, account, _, _) = session();
    serve(&mut s, account, "", vec![own_meeting("m-1", "Planning", at(12, 13, 0))]);
    let planning = event_called(&s, "m-1").id;
    // How much it matters and whether another time may be proposed, in one save.
    run(&mut s, "calendar.edit", json!({"id": planning, "importance": "high", "allowProposals": false, "send": true}));
    let upload = event_upload(&s, account).unwrap();
    assert_eq!((upload.ops[0].fields.importance, upload.ops[0].fields.allow_proposals), (Some(Importance::High), Some(false)));
    // The server took the change without the option: only that is put back, and it is said.
    let mut answer = said(&upload, EventResult::Done, "");
    answer.outcomes[0].new_times = Some(false);
    let applied = apply_event_outcomes(&mut s, &answer).unwrap();
    assert_eq!(
        applied.messages,
        [
            "The account's server doesn't let it be changed, once a meeting is made, whether another time may be proposed for \"Planning\". That option is as it was."
        ]
    );
    let here = s.mailbox.event(planning).unwrap();
    assert_eq!((here.importance, here.response_options.allow_proposals), (Importance::High, true));
    assert!(s.mailbox.event_changes.is_empty(), "nothing is sent a second time for it");
    // The option can still be set: a server that takes it leaves it changed, and says nothing.
    run(&mut s, "calendar.responseOptions", json!({"id": planning, "allowProposals": false, "send": true}));
    let applied = push(&mut s, account, EventResult::Done, "");
    assert!(applied.messages.is_empty() && !s.mailbox.event(planning).unwrap().response_options.allow_proposals);
    assert!(s.mailbox.event_changes.is_empty());
}
