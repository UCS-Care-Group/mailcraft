// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a meeting has beyond its time and its people, against the stand-in: how much it matters,
//! what the people invited are asked for, Teams meetings and the link to join by, proposals of
//! another time, and forwarding. Nothing here leaves this machine.

use mailcraft_model::remote::RemoteProposal;
use mailcraft_model::{Importance, OnlineMeeting, OnlineProvider};
use serde_json::json;

use super::*;

const JOIN: &str = "https://teams.example.org/l/meetup-join/19%3ameeting_abc%40thread.v2/0?context=%7b%22Tid%22%3a%221%22%7d";

#[test]
fn importance_response_options_and_online_meetings_are_read() {
    let with = |id: &str, more: Value| {
        let mut entry = event(id, "x");
        for (name, value) in more.as_object().unwrap() {
            entry[name] = value.clone();
        }
        entry
    };
    let proposed =
        |start: &str, end: &str, zone: &str| json!({"start": {"dateTime": start, "timeZone": zone}, "end": {"dateTime": end, "timeZone": zone}});
    let entries = vec![
        // As Graph says a Teams meeting the owner organizes, with somebody who proposed another time.
        with(
            "e-teams",
            json!({
                "importance": "high", "responseRequested": false, "allowNewTimeProposals": false, "hasAttachments": true,
                "isOnlineMeeting": true, "onlineMeetingProvider": "teamsForBusiness", "onlineMeetingUrl": null,
                "onlineMeeting": {"joinUrl": JOIN, "conferenceId": "177513992", "tollNumber": "+1 425 555 0123"},
                "attendees": [
                    {"type": "required", "status": {"response": "tentativelyAccepted"}, "emailAddress": {"name": "Bo Marsh", "address": "bo@example.org"},
                     "proposedNewTime": proposed("2026-10-13T13:00:00.0000000", "2026-10-13T14:00:00.0000000", "UTC")},
                    {"type": "optional", "status": {"response": "none"}, "emailAddress": {"name": "Cy", "address": "cy@example.org"}},
                    // A time said in another zone is the moment it is there: one in the afternoon on the Pacific coast.
                    {"type": "required", "emailAddress": {"address": "dee@example.org"}, "proposedNewTime": proposed("2026-10-13T13:00:00", "2026-10-13T14:00:00", "Pacific Standard Time")},
                    // One in a zone nobody knows is not guessed at; nor is what isn't a time at all.
                    {"type": "required", "emailAddress": {"address": "fay@example.org"}, "proposedNewTime": proposed("2026-10-13T13:00:00", "2026-10-13T14:00:00", "Olympus Mons Time")},
                    {"type": "required", "emailAddress": {"address": "eve@example.org"}, "proposedNewTime": "tomorrow"},
                ],
            }),
        ),
        // The usual: nothing of it said, or said as it is unless said otherwise.
        event("e-plain", "x"),
        with(
            "e-low",
            json!({"importance": "LOW", "responseRequested": true, "allowNewTimeProposals": null, "isOnlineMeeting": false, "onlineMeeting": null}),
        ),
        // An older online meeting, which has its link in another place.
        with(
            "e-skype",
            json!({"isOnlineMeeting": true, "onlineMeetingProvider": "skypeForBusiness", "onlineMeetingUrl": "https://meet.example.org/ada/XYZ"}),
        ),
        // Online, with nothing to join by yet; and with what isn't a link to follow.
        with("e-soon", json!({"isOnlineMeeting": true, "onlineMeetingProvider": "somethingNew", "onlineMeeting": {"joinUrl": ""}})),
        with(
            "e-odd",
            json!({"importance": 7, "isOnlineMeeting": "yes", "onlineMeeting": {"joinUrl": "javascript:alert(1)"}, "onlineMeetingUrl": "http://plain.example.org/x"}),
        ),
        with(
            "e-file",
            json!({"isOnlineMeeting": true, "onlineMeetingProvider": "teamsForBusiness", "onlineMeeting": {"joinUrl": "file:///C:/Windows/System32/calc.exe"}}),
        ),
        with("e-long", json!({"isOnlineMeeting": true, "onlineMeeting": {"joinUrl": format!("https://teams.example.org/{}", "a".repeat(5_000))}})),
    ];
    let server = FakeHttp::answering(page(entries, None));
    let (from, to) = october();
    let download = events_at(&server.url(""), TOKEN, from, to).unwrap();
    assert!(download.has_options && download.has_categories && download.complete);
    let (_, query) = asked_for(&server.request(0));
    let select = query.iter().find(|(name, _)| name == "$select").map(|(_, value)| value.as_str()).unwrap();
    for field in
        ["importance", "responseRequested", "allowNewTimeProposals", "isOnlineMeeting", "onlineMeetingProvider", "onlineMeeting", "hasAttachments"]
    {
        assert!(select.split(',').any(|asked| asked == field), "{field} in {select}");
    }
    let by_id = |id: &str| download.events.iter().find(|e| e.id == id).unwrap_or_else(|| panic!("no {id}"));

    let teams = by_id("e-teams");
    assert_eq!(
        (teams.importance, teams.options.request_responses, teams.options.allow_proposals, teams.attachments),
        (Importance::High, false, false, true)
    );
    assert_eq!(teams.online, Some(OnlineMeeting { provider: OnlineProvider::Teams, join_url: JOIN.into() }));
    assert_eq!(
        teams.proposals,
        [
            RemoteProposal { from: Address::new("Bo Marsh", "bo@example.org"), start: at(2026, 10, 13, 13, 0), end: at(2026, 10, 13, 14, 0) },
            RemoteProposal { from: Address::new("", "dee@example.org"), start: at(2026, 10, 13, 20, 0), end: at(2026, 10, 13, 21, 0) },
        ]
    );
    let plain = by_id("e-plain");
    assert_eq!(
        (plain.importance, plain.options.request_responses, plain.options.allow_proposals, plain.attachments),
        (Importance::Normal, true, true, false)
    );
    assert!(plain.online.is_none() && plain.proposals.is_empty());
    let low = by_id("e-low");
    assert_eq!((low.importance, low.options.request_responses, low.options.allow_proposals, &low.online), (Importance::Low, true, true, &None));
    assert_eq!(
        by_id("e-skype").online,
        Some(OnlineMeeting { provider: OnlineProvider::SkypeForBusiness, join_url: "https://meet.example.org/ada/XYZ".into() })
    );
    assert_eq!(by_id("e-soon").online, Some(OnlineMeeting { provider: OnlineProvider::Other, join_url: String::new() }));
    // Only an https address is a link to join by: anything else is none, and nothing is followed.
    assert_eq!((by_id("e-odd").importance, &by_id("e-odd").online), (Importance::Normal, &None));
    assert_eq!(by_id("e-file").online, Some(OnlineMeeting { provider: OnlineProvider::Teams, join_url: String::new() }));
    assert_eq!(by_id("e-long").online.as_ref().map(|online| online.join_url.as_str()), Some(""));
}

#[test]
fn importance_response_options_and_teams_are_written() {
    let server = FakeHttp::start(|n, _| {
        match n {
        0 => json_answer(
            "201 Created",
            &json!({"id": "made", "changeKey": "ck", "isOnlineMeeting": true, "onlineMeetingProvider": "teamsForBusiness", "onlineMeeting": {"joinUrl": JOIN}}).to_string(),
        ),
        _ => json_answer("200 OK", r#"{"id":"e-1","changeKey":"ck-2"}"#),
    }
    });
    let meeting = EventFields {
        title: Some("Sync".into()),
        attendees: Some(vec![Attendee { address: Address::new("Bo", "bo@example.org"), ..Default::default() }]),
        importance: Some(Importance::High),
        request_responses: Some(false),
        allow_proposals: Some(false),
        online: Some(OnlineProvider::Teams),
        ..Default::default()
    };
    let back = EventFields { importance: Some(Importance::Normal), request_responses: Some(true), allow_proposals: Some(true), ..Default::default() };
    let uploaded = push_events_at(&server.url(""), TOKEN, &upload(vec![create(meeting), update("e-1", back)]), &EventOptions::default());
    assert_eq!(
        body_of(&server.request(0)),
        json!({
            "subject": "Sync",
            "attendees": [{"emailAddress": {"address": "bo@example.org", "name": "Bo"}, "type": "required"}],
            "importance": "high",
            "responseRequested": false,
            "allowNewTimeProposals": false,
            "isOnlineMeeting": true,
            "onlineMeetingProvider": "teamsForBusiness",
            "transactionId": MARK,
        })
    );
    assert_eq!(body_of(&server.request(1)), json!({"importance": "normal", "responseRequested": true, "allowNewTimeProposals": true}));
    // The link to join by comes back with the answer to making it.
    let made = &uploaded.outcomes[0];
    assert_eq!((made.result, made.online, made.join_url.as_str(), made.error.as_str()), (EventResult::Done, Some(true), JOIN, ""));
    // An answer about an event that wasn't to be held online says nothing of it.
    assert_eq!((uploaded.outcomes[1].online, uploaded.outcomes[1].join_url.as_str()), (None, ""));
}

#[test]
fn an_account_that_cannot_make_a_teams_meeting_gets_the_meeting_without_one() {
    // Graph won't have the online meeting (400): the same event is asked for without it, and the
    // outcome says that it is not online, with what Graph said.
    let refusal = || answer("400 Bad Request", "", &graph_error("ErrorNotSupported", "The provider isn't allowed for this calendar."));
    let server = FakeHttp::start(move |n, _| match n {
        0 => refusal(),
        _ => json_answer("201 Created", r#"{"id":"made","changeKey":"ck","isOnlineMeeting":false}"#),
    });
    let meeting = EventFields {
        title: Some("Sync".into()),
        attendees: Some(vec![Attendee { address: Address::new("Bo", "bo@example.org"), ..Default::default() }]),
        online: Some(OnlineProvider::Teams),
        ..Default::default()
    };
    let uploaded = push_events_at(&server.url(""), TOKEN, &upload(vec![create(meeting.clone())]), &EventOptions::default());
    assert_eq!(server.asked(), 2);
    assert_eq!(body_of(&server.request(0))["isOnlineMeeting"], true);
    let again = body_of(&server.request(1));
    assert!(again.get("isOnlineMeeting").is_none() && again.get("onlineMeetingProvider").is_none(), "{again}");
    assert_eq!((again["subject"].as_str(), again["transactionId"].as_str()), (Some("Sync"), Some(MARK)));
    let made = &uploaded.outcomes[0];
    assert_eq!((made.result, made.remote.as_str(), made.online, made.join_url.as_str()), (EventResult::Done, "made", Some(false), ""));
    assert_eq!(made.error, "Microsoft Graph answered with status 400 (ErrorNotSupported)");

    // Graph takes the event and says nothing came of the online meeting: that is what the outcome says.
    let server = FakeHttp::answering(json_answer("201 Created", r#"{"id":"made","isOnlineMeeting":false,"onlineMeeting":null}"#));
    let uploaded = push_events_at(&server.url(""), TOKEN, &upload(vec![create(meeting.clone())]), &EventOptions::default());
    assert_eq!((server.asked(), uploaded.outcomes[0].result, uploaded.outcomes[0].online), (1, EventResult::Done, Some(false)));

    // A meeting that only was to become a Teams meeting, which Graph won't have: nothing is left
    // to change, and that is the answer.
    let server = FakeHttp::answering(refusal());
    let only = update("e-1", EventFields { online: Some(OnlineProvider::Teams), ..Default::default() });
    let uploaded = push_events_at(&server.url(""), TOKEN, &upload(vec![only]), &EventOptions::default());
    let said = &uploaded.outcomes[0];
    assert_eq!((server.asked(), said.result, said.remote.as_str(), said.online), (1, EventResult::Done, "e-1", Some(false)));

    // A refusal that isn't about the online meeting stays a refusal: it is asked without it once,
    // and what Graph says then is the answer.
    let server = FakeHttp::answering(answer("400 Bad Request", "", &graph_error("ErrorInvalidRequest", "No.")));
    let uploaded = push_events_at(&server.url(""), TOKEN, &upload(vec![create(meeting)]), &EventOptions::default());
    assert_eq!((server.asked(), uploaded.outcomes[0].result, uploaded.outcomes[0].online), (2, EventResult::Refused, None));

    // What comes back as a link to join by is one, or it is none.
    let server =
        FakeHttp::answering(json_answer("201 Created", r#"{"id":"made","isOnlineMeeting":true,"onlineMeeting":{"joinUrl":"javascript:alert(1)"}}"#));
    let online = create(EventFields { title: Some("x".into()), online: Some(OnlineProvider::Teams), ..Default::default() });
    let uploaded = push_events_at(&server.url(""), TOKEN, &upload(vec![online]), &EventOptions::default());
    assert_eq!((uploaded.outcomes[0].online, uploaded.outcomes[0].join_url.as_str()), (Some(true), ""));
}

#[test]
fn another_time_is_proposed_with_an_answer_that_is_sent() {
    let server = FakeHttp::answering(ACCEPTED.to_vec());
    let time = EventTime { start: at(2026, 10, 13, 16, 0), end: at(2026, 10, 13, 17, 0), all_day: false };
    let respond = |response: Response, send: bool| EventOp {
        kind: EventChangeKind::Respond,
        event: 9,
        remote: "e-9".into(),
        response,
        send,
        comment: "Can you make the next day instead?".into(),
        proposed: Some(time),
        ..Default::default()
    };
    let ops = vec![
        respond(Response::Tentative, true),
        respond(Response::Declined, true),
        respond(Response::Accepted, true),
        respond(Response::Tentative, false),
    ];
    let uploaded = push_events_at(&server.url(""), TOKEN, &upload(ops), &EventOptions { zone: "Pacific Standard Time".into(), ..Default::default() });
    assert_eq!((server.asked(), uploaded.outcomes.iter().filter(|o| o.result == EventResult::Done).count()), (4, 4));
    let proposed = json!({
        "start": {"dateTime": "2026-10-13T16:00:00", "timeZone": "UTC"},
        "end": {"dateTime": "2026-10-13T17:00:00", "timeZone": "UTC"},
    });
    assert!(server.request(0).starts_with("POST /v1.0/me/events/e-9/tentativelyAccept HTTP/1.1\r\n"), "{}", server.request(0));
    assert_eq!(
        body_of(&server.request(0)),
        json!({"comment": "Can you make the next day instead?", "sendResponse": true, "proposedNewTime": proposed})
    );
    assert!(server.request(1).starts_with("POST /v1.0/me/events/e-9/decline HTTP/1.1\r\n"), "{}", server.request(1));
    assert_eq!(body_of(&server.request(1))["proposedNewTime"], proposed);
    // A plain yes proposes nothing; and nothing is proposed with an answer that isn't sent
    // (Graph refuses that, and it would be mail nobody was asked about).
    assert!(server.request(2).starts_with("POST /v1.0/me/events/e-9/accept HTTP/1.1\r\n"));
    assert_eq!(body_of(&server.request(2)), json!({"comment": "Can you make the next day instead?", "sendResponse": true}));
    assert_eq!(body_of(&server.request(3)), json!({"comment": "", "sendResponse": false}));

    // The organizer doesn't take proposals (400): refused for good, in Graph's words.
    let server = FakeHttp::answering(answer("400 Bad Request", "", &graph_error("ErrorInvalidRequest", "Proposals are not allowed.")));
    let uploaded = push_events_at(&server.url(""), TOKEN, &upload(vec![respond(Response::Tentative, true)]), &EventOptions::default());
    assert_eq!((server.asked(), uploaded.outcomes[0].result), (1, EventResult::Refused));
    assert_eq!(uploaded.outcomes[0].error, "Microsoft Graph answered with status 400 (ErrorInvalidRequest)");
}

#[test]
fn a_meeting_is_forwarded_to_the_people_it_names_and_to_nobody_else() {
    let server = FakeHttp::answering(ACCEPTED.to_vec());
    let forward = |to: Vec<Address>, send: bool| EventOp {
        kind: EventChangeKind::Forward,
        event: 9,
        remote: "e/9=".into(),
        token: MARK.into(),
        to,
        send,
        comment: "Dana, hope you can make this.".into(),
        ..Default::default()
    };
    let people = vec![
        Address::new("Dana Swope", "dana@example.org"),
        Address::new("", "eli@example.org"),
        // Nobody twice, and nobody without an address that is one.
        Address::new("Dana again", "DANA@example.org"),
        Address::new("Room 4", ""),
        Address::new("Bcc", "x@example.org\r\nBcc: eve@example.org"),
    ];
    let uploaded = push_events_at(&server.url(""), TOKEN, &upload(vec![forward(people.clone(), true)]), &EventOptions::default());
    assert_eq!((server.asked(), uploaded.outcomes[0].result), (1, EventResult::Done));
    let request = server.request(0);
    assert!(request.starts_with("POST /v1.0/me/events/e%2F9%3D/forward HTTP/1.1\r\n"), "{request}");
    assert!(request.contains(&format!("Authorization: Bearer {TOKEN}\r\n")) && request.contains("Content-Type: application/json\r\n"), "{request}");
    assert_eq!(
        body_of(&request),
        json!({
            "ToRecipients": [
                {"EmailAddress": {"Address": "dana@example.org", "Name": "Dana Swope"}},
                {"EmailAddress": {"Address": "eli@example.org", "Name": ""}},
            ],
            "Comment": "Dana, hope you can make this.",
        })
    );

    // Without the owner's yes, without anybody to send it to, without a meeting: Graph is not asked.
    for (op, why) in [
        (forward(people.clone(), false), "the forward doesn't say which meeting goes to whom"),
        (forward(Vec::new(), true), "the forward doesn't say which meeting goes to whom"),
        (forward(vec![Address::new("Room 4", "")], true), "the forward doesn't say which meeting goes to whom"),
        (EventOp { remote: String::new(), ..forward(people.clone(), true) }, "the forward doesn't say which meeting goes to whom"),
    ] {
        let server = FakeHttp::answering(ACCEPTED.to_vec());
        let uploaded = push_events_at(&server.url(""), TOKEN, &upload(vec![op]), &EventOptions::default());
        assert_eq!((server.asked(), uploaded.outcomes[0].result, uploaded.outcomes[0].error.as_str()), (0, EventResult::Refused, why));
    }

    // What Graph says to a forward it doesn't carry out.
    for (status, result) in [
        ("404 Not Found", EventResult::Gone),
        ("403 Forbidden", EventResult::Denied),
        ("400 Bad Request", EventResult::Refused),
        ("503 Service Unavailable", EventResult::Failed),
        ("500 Internal Server Error", EventResult::Failed),
    ] {
        let server = FakeHttp::answering(answer(status, "", &graph_error("ErrorSomething", &format!("{TOKEN} dana@example.org"))));
        let uploaded = push_events_at(&server.url(""), TOKEN, &upload(vec![forward(people.clone(), true)]), &EventOptions::default());
        let said = &uploaded.outcomes[0];
        assert_eq!((server.asked(), said.result), (1, result), "{status}");
        // Nothing of who it was for, or of the sign-in, comes back in what is said.
        assert!(!said.error.contains("dana") && !said.error.contains("tok.en"), "{}", said.error);
    }
    // The sign-in is no longer taken: nothing more goes with it.
    let server = FakeHttp::answering(answer("401 Unauthorized", "", "{}"));
    let uploaded =
        push_events_at(&server.url(""), TOKEN, &upload(vec![forward(people.clone(), true), forward(people, true)]), &EventOptions::default());
    assert!(uploaded.token_refused && uploaded.outcomes.is_empty() && server.asked() == 1);

    // One occurrence of a series is found by its time first, and then forwarded as itself.
    let server = FakeHttp::start(|n, _| match n {
        0 => page(
            vec![
                json!({"id": "occ-1", "isAllDay": false, "originalStart": "2026-10-19T13:00:00Z", "start": {"dateTime": "2026-10-19T13:00:00", "timeZone": "UTC"}}),
            ],
            None,
        ),
        _ => ACCEPTED.to_vec(),
    });
    let one = EventOp {
        remote: String::new(),
        series: "s-1".into(),
        occurrence: at(2026, 10, 19, 13, 0),
        ..forward(vec![Address::new("Dana", "dana@example.org")], true)
    };
    let uploaded = push_events_at(&server.url(""), TOKEN, &upload(vec![one]), &EventOptions::default());
    assert_eq!((server.asked(), uploaded.outcomes[0].result), (2, EventResult::Done));
    assert!(server.request(0).starts_with("GET /v1.0/me/events/s-1/instances?"), "{}", server.request(0));
    assert!(server.request(1).starts_with("POST /v1.0/me/events/occ-1/forward HTTP/1.1\r\n"), "{}", server.request(1));
}

#[test]
fn a_change_refused_for_what_it_says_of_proposing_another_time_goes_without_that() {
    let refusal = || answer("400 Bad Request", "", &graph_error("ErrorInvalidPropertySet", "The property 'allowNewTimeProposals' is read-only."));
    let taken = || json_answer("200 OK", r#"{"id":"e-1","changeKey":"ck-2"}"#);
    let push = |server: &FakeHttp, op: &EventOp| {
        push_events_at(&server.url(""), TOKEN, &upload(vec![op.clone()]), &EventOptions::default()).outcomes.remove(0)
    };
    // How much it matters and that option in one change. Graph refuses, and is asked once more
    // without the option: the rest is taken, and the outcome says what was left out.
    let both = update("e-1", EventFields { importance: Some(Importance::High), allow_proposals: Some(false), ..Default::default() });
    let server = FakeHttp::start(move |n, _| match n {
        0 => refusal(),
        _ => taken(),
    });
    let said = push(&server, &both);
    assert_eq!(server.asked(), 2);
    assert_eq!(body_of(&server.request(0)), json!({"importance": "high", "allowNewTimeProposals": false}));
    assert_eq!(body_of(&server.request(1)), json!({"importance": "high"}));
    assert_eq!((said.result, said.remote.as_str(), said.change_key.as_str(), said.new_times), (EventResult::Done, "e-1", "ck-2", Some(false)));
    // A Graph that takes it is asked once, and nothing is said of leaving anything out.
    let server = FakeHttp::answering(taken());
    let said = push(&server, &both);
    assert_eq!((server.asked(), said.result, said.new_times), (1, EventResult::Done, None));
    // Only that option, which Graph won't have changed: nothing is left to ask for.
    let server = FakeHttp::answering(refusal());
    let said = push(&server, &update("e-1", EventFields { allow_proposals: Some(true), ..Default::default() }));
    assert_eq!((server.asked(), said.result, said.remote.as_str(), said.new_times), (1, EventResult::Done, "e-1", Some(false)));

    // A refusal for another reason stays a refusal. A change that doesn't say that option is
    // not asked a second time at all...
    let no = || answer("400 Bad Request", "", &graph_error("ErrorInvalidRequest", "No."));
    let server = FakeHttp::answering(no());
    let said = push(&server, &update("e-1", EventFields { importance: Some(Importance::High), ..Default::default() }));
    assert_eq!((server.asked(), said.result, said.new_times), (1, EventResult::Refused, None));
    // ...and one that does, and is refused without it as well, is refused: nothing of it is
    // called taken.
    let server = FakeHttp::answering(no());
    let said = push(&server, &both);
    assert_eq!((server.asked(), said.result, said.new_times), (2, EventResult::Refused, None));
    // A new meeting says the option like everything else it says, and is not asked twice for it.
    let server = FakeHttp::answering(no());
    let said = push(&server, &create(EventFields { title: Some("Sync".into()), allow_proposals: Some(false), ..Default::default() }));
    assert_eq!((server.asked(), said.result, said.new_times), (1, EventResult::Refused, None));
}
