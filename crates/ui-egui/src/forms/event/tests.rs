// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The appointment and meeting form.

use super::*;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn session() -> Session {
    Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0)
}

fn day(y: i32, m: u8, d: u8) -> Date {
    Date::new(y, m, d).unwrap()
}

fn event_id(s: &Session, title: &str) -> Id {
    s.mailbox.events.values().find(|e| e.title == title).map(|e| e.id).unwrap()
}

/// The form's four date and time fields.
fn span(f: &EventForm) -> (&str, &str, &str, &str) {
    (&f.start_date, &f.start_time, &f.end_date, &f.end_time)
}

#[test]
fn a_date_knows_which_of_its_weekday_it_is() {
    assert_eq!(nth_weekday(day(2026, 10, 14)), (2, Weekday::Wed));
    assert_eq!(nth_weekday(day(2026, 10, 1)), (1, Weekday::Thu));
    assert_eq!(nth_weekday(day(2026, 10, 21)), (3, Weekday::Wed));
    assert_eq!(nth_weekday(day(2026, 10, 30)), (-1, Weekday::Fri));
    assert_eq!(nth_choices(day(2026, 10, 14)), [2]);
    // A fourth one that is also the last can be counted either way; a fifth is the last.
    assert_eq!(nth_weekday(day(2026, 10, 28)), (-1, Weekday::Wed));
    assert_eq!(nth_choices(day(2026, 10, 28)), [4, -1]);
    assert_eq!(nth_choices(day(2026, 10, 29)), [-1]);
    // February 2026 has four of every day.
    assert_eq!(nth_weekday(day(2026, 2, 21)), (3, Weekday::Sat));
    assert_eq!(nth_choices(day(2026, 2, 22)), [4, -1]);
    // A count that fits the date stays; one that doesn't is read from the date.
    assert_eq!((fit_nth(day(2026, 10, 28), 4), fit_nth(day(2026, 10, 28), 2), fit_nth(day(2026, 10, 14), -1)), (4, -1, 2));
    assert_eq!((ordinal(2), ordinal(-1), ordinal(i8::MIN)), ("second", "last", "last"));
}

#[test]
fn a_new_appointment_starts_where_the_command_would_put_it() {
    let mut s = session();
    let f = EventForm::new(&s, &json!({}), false);
    // The next half hour today.
    assert_eq!(span(&f), ("2026-10-14", "4:30 PM", "2026-10-14", "5:00 PM"));
    assert_eq!((f.reminder, f.all_day, f.meeting, f.minutes, f.calendar), (Some(15), false, false, 30, s.mailbox.default_calendar().unwrap()));
    assert_eq!((f.name(), f.title().as_str()), ("event", "New Appointment"));
    // Another day in view: the start of the working day.
    s.view.date = day(2026, 10, 20);
    let f = EventForm::new(&s, &json!({}), true);
    assert_eq!(span(&f), ("2026-10-20", "8:00 AM", "2026-10-20", "8:30 AM"));
    assert_eq!((f.name(), f.title().as_str(), f.meeting), ("meeting", "New Meeting", true));
    // The fields show times the way the clock preference says.
    s.prefs.twelve_hour = false;
    assert_eq!(EventForm::new(&s, &json!({}), false).start_time, "08:00");
    s.prefs.default_reminder = None;
    assert_eq!(EventForm::new(&s, &json!({}), false).reminder, None);
}

#[test]
fn a_new_event_takes_what_the_params_say() {
    let s = session();
    let f = EventForm::new(&s, &json!({"start": "2026-10-16T15:00", "title": "Review", "location": "Room 4"}), false);
    assert_eq!((f.title.as_str(), f.location.as_str()), ("Review", "Room 4"));
    assert_eq!(span(&f), ("2026-10-16", "3:00 PM", "2026-10-16", "3:30 PM"));
    let f = EventForm::new(&s, &json!({"start": "2026-10-16T15:00", "end": "2026-10-17T09:15"}), false);
    assert_eq!((span(&f), f.minutes), (("2026-10-16", "3:00 PM", "2026-10-17", "9:15 AM"), 18 * 60 + 15));
    // A day alone gets the time the command would pick on that day.
    assert_eq!(span(&EventForm::new(&s, &json!({"start": "2026-10-16"}), false)), ("2026-10-16", "8:00 AM", "2026-10-16", "8:30 AM"));
    assert_eq!(span(&EventForm::new(&s, &json!({"start": "2026-10-14"}), false)), ("2026-10-14", "4:30 PM", "2026-10-14", "5:00 PM"));
    // Whole days: `end` is the last day; the reminder is the afternoon before, and the days show as free.
    let f = EventForm::new(&s, &json!({"start": "2026-10-16", "end": "2026-10-18", "allDay": true}), false);
    assert_eq!((f.all_day, f.start_date.as_str(), f.end_date.as_str(), f.days, f.reminder), (true, "2026-10-16", "2026-10-18", 2, Some(1080)));
    assert_eq!(f.show_as, ShowAs::Free);
    let f = EventForm::new(&s, &json!({"start": "2026-10-16T15:00", "allDay": true, "reminder": 30}), false);
    assert_eq!((f.start_date.as_str(), f.end_date.as_str(), f.days, f.reminder), ("2026-10-16", "2026-10-16", 0, Some(30)));
    // An end before the start, and nonsense, fall back to what a new event gets.
    assert_eq!(
        span(&EventForm::new(&s, &json!({"start": "2026-10-16T15:00", "end": "2026-10-16T14:00"}), false)),
        ("2026-10-16", "3:00 PM", "2026-10-16", "3:30 PM")
    );
    let f =
        EventForm::new(&s, &json!({"start": "soon", "end": 7, "allDay": "yes", "title": 3, "reminder": "x", "categories": "Blue category"}), false);
    assert_eq!((span(&f), f.title.as_str(), f.reminder, f.categories.len()), (("2026-10-14", "4:30 PM", "2026-10-14", "5:00 PM"), "", Some(15), 0));
    // Someone to invite makes it a meeting, whatever it was opened as.
    let f = EventForm::new(&s, &json!({"attendees": "maya@lumen.example", "showAs": "tentative", "private": true, "reminder": null}), false);
    assert_eq!(
        (f.meeting, f.name(), f.title().as_str(), f.show_as, f.private, f.reminder),
        (true, "event", "New Meeting", ShowAs::Tentative, true, None)
    );
}

#[test]
fn a_new_appointment_sends_every_field() {
    let s = session();
    let calendar = s.mailbox.default_calendar().unwrap();
    let mut f = EventForm::new(&s, &json!({"start": "2026-10-16T15:00"}), false);
    f.title = "Design review".into();
    f.location = "Room 4".into();
    f.categories = vec!["Blue category".into()];
    assert_eq!(
        f.command(),
        (
            "calendar.new",
            json!({
                "title": "Design review", "location": "Room 4", "description": "",
                "start": "2026-10-16T15:00", "end": "2026-10-16T15:30", "allDay": false,
                "reminder": 15, "showAs": "busy", "categories": ["Blue category"], "private": false, "calendar": calendar,
                "importance": "normal",
            })
        )
    );
    // The same as a meeting: all day, repeating, with everything set.
    f.meeting = true;
    f.required = "maya@lumen.example; Tomás <tomas@lumen.example>".into();
    f.optional = "priya@lumen.example".into();
    f.set_all_day(true, Some(15), true);
    f.repeat =
        Repeat { freq: Some(Freq::Weekly), interval: 2, days: vec![Weekday::Fri, Weekday::Mon], ends: Ends::After, count: 6, ..Default::default() };
    f.show_as = ShowAs::OutOfOffice;
    f.private = true;
    f.description = "Bring\nnotes".into();
    // And what a meeting has beyond an appointment: how much it matters, what the people
    // invited are asked for, and that it is to be a Teams meeting.
    f.importance = Importance::High;
    f.options.allow_proposals = false;
    f.online = true;
    assert_eq!(
        f.command(),
        (
            "calendar.newMeeting",
            json!({
                "title": "Design review", "location": "Room 4", "description": "Bring\nnotes",
                "start": "2026-10-16", "end": "2026-10-16", "allDay": true,
                "attendees": "maya@lumen.example; Tomás <tomas@lumen.example>", "optional": "priya@lumen.example",
                "recurrence": {"freq": "weekly", "interval": 2, "byDay": ["mon", "fri"], "count": 6},
                // A new whole-day event reminds the afternoon before (18 hours).
                "reminder": 1080, "showAs": "outOfOffice", "categories": ["Blue category"], "private": true, "calendar": calendar,
                "importance": "high", "requestResponses": true, "allowProposals": false, "online": true,
            })
        )
    );
}

#[test]
fn importance_response_options_and_teams_are_in_the_bar_and_in_what_is_saved() {
    let mut s = session();
    let key = |f: &EventForm, key: &str| f.bar_items().into_iter().find(|item| item.key == key);
    // An appointment: High and Low Importance, each on or off; nothing of a meeting's.
    let mut f = EventForm::new(&s, &json!({"start": "2026-10-16T15:00", "title": "Dentist"}), false);
    assert!(key(&f, "importanceHigh").is_some_and(|item| !item.checked) && key(&f, "importanceLow").is_some());
    assert!(key(&f, "teams").is_none() && key(&f, "responseOptions").is_none() && key(&f, "forward").is_none());
    f.importance = Importance::Low;
    assert!(key(&f, "importanceLow").is_some_and(|item| item.checked) && key(&f, "importanceHigh").is_some_and(|item| !item.checked));
    let (command, params) = f.command();
    assert_eq!((command, &params["importance"], params.get("online"), params.get("allowProposals")), ("calendar.new", &json!("low"), None, None));
    let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.event(id).unwrap().importance, Importance::Low);
    // Opened again it says so, has Forward in its bar, and sends only what changes.
    let mut f = EventForm::edit(&s, &json!({"id": id})).unwrap();
    assert!(key(&f, "importanceLow").is_some_and(|item| item.checked) && key(&f, "forward").is_some() && !f.dirty());
    f.importance = Importance::High;
    assert_eq!(f.command(), ("calendar.edit", json!({"id": id, "importance": "high"})));
    // A meeting the owner organizes: Teams Meeting and Response Options beside them.
    let mut f = EventForm::new(&s, &json!({"start": "2026-10-16T15:00", "title": "Sync"}), true);
    assert!(key(&f, "teams").is_some_and(|item| item.on && !item.checked) && key(&f, "responseOptions").is_some_and(|item| item.menu));
    f.online = true;
    assert!(key(&f, "teams").is_some_and(|item| item.checked));
    // One that its server has made a Teams meeting stays one: the button says so.
    let event = mailcraft_model::Event {
        title: "Made".into(),
        attendees: vec![mailcraft_model::Attendee { address: Address::new("Maya", "maya@lumen.example"), ..Default::default() }],
        online: Some(mailcraft_model::OnlineMeeting { join_url: "https://teams.example.org/l/meetup-join/1".into(), ..Default::default() }),
        response_options: ResponseOptions { request_responses: false, allow_proposals: true },
        proposals: vec![mailcraft_model::Proposal::default(); 2],
        ..Default::default()
    };
    let made = s.mailbox.add_event(event).unwrap();
    let f = EventForm::edit(&s, &json!({"id": made})).unwrap();
    assert_eq!(f.meeting_state(), (Importance::Normal, true, "https://teams.example.org/l/meetup-join/1"));
    assert_eq!((f.options.request_responses, f.options.allow_proposals, f.proposals, f.online_made), (false, true, 2, false));
    // (Kept here, no server made it: it can still be taken back.)
    assert!(key(&f, "teams").is_some_and(|item| item.on && item.checked));
    let made_there = EventForm { online_made: true, ..EventForm::edit(&s, &json!({"id": made})).unwrap() };
    assert!(key(&made_there, "teams").is_some_and(|item| !item.on && item.checked && item.tip.contains("stays one")));
    // Somebody else's meeting has neither: both are its organizer's to say.
    let theirs = EventForm { guest: true, ..EventForm::edit(&s, &json!({"id": made})).unwrap() };
    assert!(key(&theirs, "teams").is_none() && key(&theirs, "responseOptions").is_none() && key(&theirs, "importanceHigh").is_some());
    // Another time that was proposed goes into the fields, and is then a change to send.
    let mut f = EventForm::edit(&s, &json!({"id": made, "proposed": {"start": "2026-10-20T14:00:00", "end": "2026-10-20T15:30:00"}})).unwrap();
    assert_eq!((f.start_date.as_str(), f.end_date.as_str(), f.minutes, f.dirty()), ("2026-10-20", "2026-10-20", 90, true));
    let (_, params) = f.command();
    assert_eq!((&params["start"], &params["end"]), (&json!("2026-10-20T14:00"), &json!("2026-10-20T15:30")));
    // A proposal that is none (backwards, unreadable) changes nothing.
    f.take_proposal(DateTime::at_midnight(Date::new(2026, 10, 21).unwrap()), DateTime::at_midnight(Date::new(2026, 10, 20).unwrap()), true);
    assert_eq!(f.start_date, "2026-10-20");
    let f = EventForm::edit(&s, &json!({"id": made, "proposed": {"start": "soon", "end": 7}})).unwrap();
    assert!(!f.dirty());
}

#[test]
fn the_commands_run_and_their_errors_come_back() {
    let mut s = session();
    let mut f = EventForm::new(&s, &json!({"start": "2026-10-16T15:00", "title": "Sync"}), true);
    f.required = "maya@lumen.example".into();
    f.repeat = Repeat { freq: Some(Freq::Monthly), nth: Some(3), ..Default::default() };
    let (command, params) = f.command();
    let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
    let e = s.mailbox.event(id).unwrap();
    assert_eq!(
        (e.title.as_str(), e.start.iso().as_str(), e.duration_minutes(), e.attendees.len(), e.reminder_minutes),
        ("Sync", "2026-10-16T15:00:00", 30, 1, Some(15))
    );
    assert_eq!(e.recurrence.as_ref().map(|r| r.describe(e.start.date)).as_deref(), Some("Every month on the third Friday"));
    // Whole days: the end field names the last day.
    let mut f = EventForm::new(&s, &json!({"start": "2026-10-19", "end": "2026-10-21", "allDay": true, "title": "Offsite"}), false);
    let (command, params) = f.command();
    let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
    let e = s.mailbox.event(id).unwrap();
    assert_eq!(
        (e.all_day, e.start.iso().as_str(), e.end.iso().as_str(), e.reminder_minutes, e.show_as),
        (true, "2026-10-19T00:00:00", "2026-10-22T00:00:00", Some(1080), ShowAs::Free)
    );
    // An end before the start: the command says so and nothing is added.
    let before = s.mailbox.events.len();
    f.end_date = "2026-10-18".into();
    let (command, params) = f.command();
    assert_eq!(s.run(command, &params).unwrap_err().to_string(), "the end is before the start");
    let mut f = EventForm::new(&s, &json!({"start": "2026-10-16T15:00", "title": "Backwards"}), false);
    f.end_time = "1pm".into();
    let (command, params) = f.command();
    assert_eq!(s.run(command, &params).unwrap_err().to_string(), "the end is before the start");
    // Someone who isn't an address.
    f.end_time = "4pm".into();
    f.meeting = true;
    f.required = "maya".into();
    let (command, params) = f.command();
    assert_eq!(s.run(command, &params).unwrap_err().to_string(), "\"maya\" isn't an e-mail address");
    assert_eq!(s.mailbox.events.len(), before);
}

#[test]
fn time_zones_keep_the_clock_time_and_go_with_the_times_they_are_for() {
    let zone = |name: &str| Zone::named(name).unwrap();
    // The calendar is shown in Chicago's time.
    let mut s = Session::in_zone(mailcraft_model::Mailbox::with_local_account("", ""), NOW, zone("America/Chicago"));
    let mut f = EventForm::new(&s, &json!({"title": "Call", "start": "2026-10-13T15:00", "end": "2026-10-13T16:00"}), false);
    // Without Time Zones nothing is said of zones, as ever.
    assert!(!f.zones && f.command().1.get("startZone").is_none());
    assert!(f.bar_items().iter().any(|item| item.key == "timeZones" && !item.checked && item.on));
    // Turned on, the start and the end say the calendar's own zone.
    f.set_zones(true);
    assert_eq!((f.start_zone.as_str(), f.end_zone.as_str()), ("America/Chicago", "America/Chicago"));
    assert!(f.bar_items().iter().any(|item| item.key == "timeZones" && item.checked));
    // Another zone for the start: the time on the clock stays (and so the event moves),
    // and the end's zone goes with the start's.
    f.pick_zone(zone("America/New_York"), false);
    assert_eq!(span(&f), ("2026-10-13", "3:00 PM", "2026-10-13", "4:00 PM"));
    let (command, params) = f.command();
    assert_eq!(command, "calendar.new");
    assert_eq!(
        (params["start"].as_str(), params["startZone"].as_str(), params["endZone"].is_null()),
        (Some("2026-10-13T15:00"), Some("America/New_York"), true)
    );
    let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
    let made = s.mailbox.event(id).unwrap().clone();
    assert_eq!((made.start.iso(), made.start_zone.as_str()), ("2026-10-13T14:00:00".to_string(), "America/New_York"));
    assert_eq!(s.prefs.recent_zones, Vec::<String>::new());

    // Opened again, it shows its own clock and its zones, and nothing is to be sent.
    let mut f = EventForm::edit(&s, &json!({"id": id})).unwrap();
    assert!(f.zones && f.had_zones);
    assert_eq!(
        (span(&f), f.start_zone.as_str(), f.end_zone.as_str()),
        (("2026-10-13", "3:00 PM", "2026-10-13", "4:00 PM"), "America/New_York", "America/New_York")
    );
    assert_eq!(f.command(), ("calendar.edit", json!({"id": id})));
    // Opened to take up another time that somebody proposed, which is said by the clock
    // here: the fields show that moment by the event's own clock (four here is five there).
    let proposed = json!({"start": "2026-10-14T16:00:00", "end": "2026-10-14T17:00:00"});
    let taken = EventForm::edit(&s, &json!({"id": id, "proposed": proposed})).unwrap();
    assert_eq!(span(&taken), ("2026-10-14", "5:00 PM", "2026-10-14", "6:00 PM"));
    // Another title is only that.
    f.title = "Call with Bo".into();
    assert_eq!(f.command().1, json!({"id": id, "title": "Call with Bo"}));
    // Another time goes with the zone it is by.
    f.start_time = "5:00 PM".into();
    f.start_moved(true);
    assert_eq!(
        f.command().1,
        json!({"id": id, "title": "Call with Bo", "start": "2026-10-13T17:00", "end": "2026-10-13T18:00", "allDay": false, "startZone": "America/New_York", "endZone": null})
    );
    // The end in a zone of its own: how long it is, is counted in instants; and moving
    // the start keeps that length.
    // (Six in the evening in Berlin is before five in New York: no length to keep yet.)
    f.pick_zone(zone("Europe/Berlin"), true);
    assert_eq!((f.start_zone.as_str(), f.end_zone.as_str(), f.minutes, f.length()), ("America/New_York", "Europe/Berlin", 60, None));
    f.end_date = "2026-10-14".into();
    f.end_time = "7:00 AM".into();
    f.end_moved();
    assert_eq!((f.minutes, f.length().as_deref()), (8 * 60, Some("8 hours")));
    f.start_time = "6:00 PM".into();
    f.start_moved(true);
    assert_eq!(span(&f), ("2026-10-13", "6:00 PM", "2026-10-14", "8:00 AM"));
    let params = f.command().1;
    assert_eq!((params["startZone"].as_str(), params["endZone"].as_str()), (Some("America/New_York"), Some("Europe/Berlin")));
    s.run("calendar.edit", &params).unwrap();
    let flight = s.mailbox.event(id).unwrap().clone();
    assert_eq!((flight.start.iso(), flight.end.iso()), ("2026-10-13T17:00:00".to_string(), "2026-10-14T01:00:00".to_string()));
    // Time Zones turned off: no zones any more, and the times as they stand on the clock.
    let mut f = EventForm::edit(&s, &json!({"id": id})).unwrap();
    f.set_zones(false);
    let params = f.command().1;
    assert_eq!((params["start"].as_str(), params["startZone"].is_null(), params["endZone"].is_null()), (Some("2026-10-13T18:00"), true, true));
    s.run("calendar.edit", &params).unwrap();
    let plain = s.mailbox.event(id).unwrap().clone();
    assert_eq!(
        (plain.start.iso(), plain.end.iso(), plain.start_zone.as_str()),
        ("2026-10-13T18:00:00".to_string(), "2026-10-14T08:00:00".to_string(), "")
    );
    // Whole days have no zones: the button is off, and none are sent.
    let mut f = EventForm::edit(&s, &json!({"id": id})).unwrap();
    f.set_zones(true);
    f.set_all_day(true, None, true);
    assert!(f.bar_items().iter().any(|item| item.key == "timeZones" && !item.checked && !item.on));
    assert!(f.command().1.get("startZone").is_none());
}

#[test]
fn an_open_form_follows_the_calendar_to_another_zone() {
    let zone = |name: &str| Zone::named(name).unwrap();
    let (chicago, london) = (zone("America/Chicago"), zone("Europe/London"));
    let mut s = Session::in_zone(mailcraft_model::Mailbox::with_local_account("", ""), NOW, chicago);
    let id =
        s.run("calendar.new", &json!({"title": "Lunch", "start": "2026-10-13T12:00", "end": "2026-10-13T13:00"})).unwrap()["id"].as_u64().unwrap();
    let abroad =
        s.run("calendar.new", &json!({"title": "Call", "start": "2026-10-13T15:00", "startZone": "Asia/Tokyo"})).unwrap()["id"].as_u64().unwrap();
    let (mut lunch, mut call) = (EventForm::edit(&s, &json!({"id": id})).unwrap(), EventForm::edit(&s, &json!({"id": abroad})).unwrap());
    let mut typed = EventForm::edit(&s, &json!({"id": id})).unwrap();
    typed.title = "Long lunch".into();
    typed.end_time = "2:00 PM".into();
    typed.end_moved();
    // The computer is in London now; the mailbox and the forms are told.
    s.clock.zone = Some(london);
    assert!(s.settle_zone());
    for form in [&mut lunch, &mut call, &mut typed] {
        form.rezone(chicago, london, true);
    }
    // Lunch reads by London's clock, as the calendar shows it, and saving it changes nothing.
    assert_eq!(span(&lunch), ("2026-10-13", "6:00 PM", "2026-10-13", "7:00 PM"));
    assert!(!lunch.dirty());
    assert_eq!(lunch.command(), ("calendar.edit", json!({"id": id})));
    // The call is by Tokyo's clock, here as there.
    assert_eq!((span(&call), call.start_zone.as_str()), (("2026-10-13", "3:00 PM", "2026-10-13", "3:30 PM"), "Asia/Tokyo"));
    assert_eq!(call.command(), ("calendar.edit", json!({"id": abroad})));
    // What was typed stays typed, at the instant it was typed for.
    assert_eq!(span(&typed), ("2026-10-13", "6:00 PM", "2026-10-13", "8:00 PM"));
    let params = typed.command().1;
    assert_eq!(
        (params["title"].as_str(), params["start"].as_str(), params["end"].as_str()),
        (Some("Long lunch"), Some("2026-10-13T18:00"), Some("2026-10-13T20:00"))
    );
    s.run("calendar.edit", &params).unwrap();
    let saved = s.mailbox.event(id).unwrap();
    assert_eq!(
        (london.instant(saved.start), london.instant(saved.end)),
        (chicago.instant(DateTime::new(day(2026, 10, 13), 12, 0, 0).unwrap()), chicago.instant(DateTime::new(day(2026, 10, 13), 14, 0, 0).unwrap()))
    );
}

#[test]
fn an_existing_event_fills_the_form() {
    let mut s = session();
    // Nothing selected and nothing named: no form.
    assert!(EventForm::edit(&s, &json!({})).is_none());
    assert!(EventForm::edit(&s, &json!({"id": 999_999})).is_none());
    let dentist = event_id(&s, "Dentist");
    s.sel.event = Some(dentist);
    let f = EventForm::edit(&s, &json!({})).unwrap();
    assert_eq!((f.name(), f.title().as_str(), f.id), ("editEvent", "Appointment", Some(dentist)));
    assert_eq!((span(&f), f.minutes), (("2026-10-17", "8:30 AM", "2026-10-17", "9:15 AM"), 45));
    assert_eq!((f.show_as, f.private, f.reminder, f.meeting, f.repeat.freq), (ShowAs::OutOfOffice, true, Some(60), false, None));
    // `id` wins over the selection. A meeting shows who is invited.
    let review = event_id(&s, "Design review: homepage");
    let f = EventForm::edit(&s, &json!({"id": review})).unwrap();
    assert_eq!((f.title().as_str(), f.meeting, f.optional.as_str()), ("Meeting", true, ""));
    assert_eq!(f.required, "Maya Okafor <maya@lumen.example>; Tomás Rivera <tomas@lumen.example>; Priya Nair <priya@lumen.example>");
    assert_eq!((f.categories.as_slice(), f.description.starts_with("Round three")), (["Blue category".to_string()].as_slice(), true));
    // Whole days: the end field is the last day, and times wait for All day to be cleared.
    let f = EventForm::edit(&s, &json!({"id": event_id(&s, "Planning days")})).unwrap();
    assert_eq!((f.all_day, span(&f), f.days), (true, ("2026-10-21", "8:00 AM", "2026-10-22", "8:30 AM"), 1));
    // Series.
    let f = EventForm::edit(&s, &json!({"id": event_id(&s, "1:1 with Priya")})).unwrap();
    assert_eq!(f.repeat, Repeat { freq: Some(Freq::Weekly), days: vec![Weekday::Tue], ..Default::default() });
    let f = EventForm::edit(&s, &json!({"id": event_id(&s, "Budget sync")})).unwrap();
    assert_eq!((f.repeat.freq, f.repeat.nth, f.repeat.ends), (Some(Freq::Monthly), Some(2), Ends::Never));
    let f = EventForm::edit(&s, &json!({"id": event_id(&s, "Dev's birthday")})).unwrap();
    assert_eq!((f.repeat.freq, f.repeat.nth, f.all_day, f.start_date.as_str()), (Some(Freq::Yearly), None, true, "2025-03-14"));
}

#[test]
fn an_edit_sends_only_what_changed() {
    let mut s = session();
    let dentist = event_id(&s, "Dentist");
    let mut f = EventForm::edit(&s, &json!({"id": dentist})).unwrap();
    assert_eq!(f.command(), ("calendar.edit", json!({"id": dentist})));
    f.title = "Dentist (check-up)".into();
    f.private = false;
    assert_eq!(f.command(), ("calendar.edit", json!({"id": dentist, "title": "Dentist (check-up)", "private": false})));
    // A new start goes with its end, which kept its distance.
    f.start_time = "10am".into();
    f.start_moved(true);
    assert_eq!(f.end_time, "10:45 AM");
    assert_eq!(
        f.command().1,
        json!({"id": dentist, "title": "Dentist (check-up)", "private": false, "start": "2026-10-17T10:00", "end": "2026-10-17T10:45", "allDay": false})
    );
    // Repeating from now on, and inviting someone.
    f.repeat.set_freq(Some(Freq::Weekly), Date::parse_iso(&f.start_date));
    assert_eq!(f.command().1["recurrence"], json!({"freq": "weekly", "interval": 1, "byDay": ["sat"]}));
    f.meeting = true;
    f.required = "maya@lumen.example".into();
    assert_eq!((&f.command().1["attendees"], &f.command().1["optional"]), (&json!("maya@lumen.example"), &json!("")));
    let (command, params) = f.command();
    s.run(command, &params).unwrap();
    let e = s.mailbox.event(dentist).unwrap();
    assert_eq!(
        (e.title.as_str(), e.private, e.start.iso().as_str(), e.duration_minutes(), e.is_meeting()),
        ("Dentist (check-up)", false, "2026-10-17T10:00:00", 45, true)
    );
    // What the form doesn't show stays as it was.
    assert_eq!((e.show_as, e.reminder_minutes), (ShowAs::OutOfOffice, Some(60)));
}

#[test]
fn ending_a_series_and_removing_attendees_say_so() {
    let mut s = session();
    let standup = event_id(&s, "Team stand-up");
    let mut f = EventForm::edit(&s, &json!({"id": standup})).unwrap();
    // A rule the form shows only in part isn't sent back unless Repeat changes.
    assert_eq!(f.command().1, json!({"id": standup}));
    f.repeat.interval = 2;
    assert_eq!(f.command().1["recurrence"], json!({"freq": "daily", "interval": 2, "byDay": ["mon", "tue", "wed", "thu", "fri"]}));
    f.repeat.set_freq(None, None);
    assert_eq!(f.command().1, json!({"id": standup, "recurrence": null}));
    let (command, params) = f.command();
    s.run(command, &params).unwrap();
    assert!(s.mailbox.event(standup).unwrap().recurrence.is_none());
    // A rule that doesn't match its start stays untouched too.
    let budget = event_id(&s, "Budget sync");
    assert_eq!(EventForm::edit(&s, &json!({"id": budget})).unwrap().command().1, json!({"id": budget}));
    // A meeting stops being one.
    let review = event_id(&s, "Design review: homepage");
    let mut f = EventForm::edit(&s, &json!({"id": review})).unwrap();
    f.meeting = false;
    assert_eq!((f.title().as_str(), f.command().1), ("Appointment", json!({"id": review, "attendees": "", "optional": ""})));
    let (command, params) = f.command();
    s.run(command, &params).unwrap();
    assert!(!s.mailbox.event(review).unwrap().is_meeting());
}

#[test]
fn moving_the_start_keeps_the_length() {
    let s = session();
    let mut f = EventForm::new(&s, &json!({"start": "2026-10-16T15:00", "end": "2026-10-16T16:30"}), false);
    assert_eq!(f.length().as_deref(), Some("1.5 hours"));
    f.start_time = "11:15pm".into();
    f.start_moved(true);
    assert_eq!(span(&f), ("2026-10-16", "11:15pm", "2026-10-17", "12:45 AM"));
    // A new end is the length from then on.
    f.end_date = "2026-10-16".into();
    f.end_time = "23:45".into();
    f.end_moved();
    assert_eq!(f.minutes, 30);
    f.start_date = "2026-11-02".into();
    f.start_moved(false);
    assert_eq!((f.end_date.as_str(), f.end_time.as_str(), f.last_start), ("2026-11-02", "23:45", Some(day(2026, 11, 2))));
    // An end before the start, or text that isn't a time, changes nothing.
    f.end_time = "9".into();
    f.end_moved();
    assert_eq!((f.minutes, f.length()), (30, None));
    f.start_time = "soon".into();
    f.start_moved(true);
    assert_eq!((f.end_time.as_str(), f.length()), ("9", None));
}

#[test]
fn all_day_covers_the_days_and_gives_the_times_back() {
    let s = session();
    let mut f = EventForm::new(&s, &json!({"start": "2026-10-16T22:00", "end": "2026-10-18T00:00"}), false);
    f.set_all_day(true, Some(15), true);
    // An end at midnight belongs to the day before. A new event's reminder becomes the
    // afternoon before, and the days show as free.
    assert_eq!((f.start_date.as_str(), f.end_date.as_str(), f.days, f.reminder), ("2026-10-16", "2026-10-17", 1, Some(1080)));
    assert_eq!(f.show_as, ShowAs::Free);
    assert_eq!(f.length().as_deref(), Some("2 days"));
    f.start_date = "2026-10-20".into();
    f.start_moved(true);
    assert_eq!(f.end_date, "2026-10-21");
    f.end_date = "2026-10-24".into();
    f.end_moved();
    assert_eq!((f.days, f.length().as_deref()), (4, Some("5 days")));
    // Back to times: the length it had before.
    f.set_all_day(false, Some(15), true);
    assert_eq!((span(&f), f.reminder), (("2026-10-20", "10:00 PM", "2026-10-22", "12:00 AM"), Some(15)));
    // A reminder chosen in the form is left alone.
    f.reminder = Some(60);
    f.reminder_set = true;
    f.set_all_day(true, Some(15), true);
    assert_eq!((f.reminder, f.end_date.as_str()), (Some(60), "2026-10-21"));
}

#[test]
fn repeat_fields_make_rules() {
    let wed = day(2026, 10, 14);
    assert_eq!(Repeat::default().rule(wed), None);
    let json = |r: &Repeat, start: Date| r.rule(start).map(|r| rule_json(&r));
    let weekly = Repeat {
        freq: Some(Freq::Weekly),
        interval: 2,
        days: vec![Weekday::Wed, Weekday::Mon, Weekday::Wed],
        ends: Ends::After,
        count: 5,
        ..Default::default()
    };
    assert_eq!(json(&weekly, wed), Some(json!({"freq": "weekly", "interval": 2, "byDay": ["mon", "wed"], "count": 5})));
    // No day picked: the start's own.
    assert_eq!(
        json(&Repeat { freq: Some(Freq::Weekly), ..Default::default() }, wed),
        Some(json!({"freq": "weekly", "interval": 1, "byDay": ["wed"]}))
    );
    let monthly = Repeat { freq: Some(Freq::Monthly), nth: Some(2), ends: Ends::Until, until: "2027-03-31".into(), ..Default::default() };
    assert_eq!(json(&monthly, wed), Some(json!({"freq": "monthly", "interval": 1, "byDay": ["wed"], "nth": 2, "until": "2027-03-31"})));
    assert_eq!(monthly.rule(wed).unwrap().describe(wed), "Every month on the second Wednesday, until Mar 31, 2027");
    assert_eq!(json(&Repeat { freq: Some(Freq::Monthly), ..Default::default() }, wed), Some(json!({"freq": "monthly", "interval": 1})));
    // The weekday and its count come from the start date.
    let yearly = Repeat { freq: Some(Freq::Yearly), nth: Some(-1), ..Default::default() };
    assert_eq!(json(&yearly, day(2026, 10, 30)), Some(json!({"freq": "yearly", "interval": 1, "byDay": ["fri"], "nth": -1})));
    assert_eq!(json(&yearly, wed), Some(json!({"freq": "yearly", "interval": 1, "byDay": ["wed"], "nth": 2})));
    // Numbers out of range are brought back in.
    let wild = Repeat { freq: Some(Freq::Daily), interval: 0, ends: Ends::After, count: u32::MAX, until: "ignored".into(), ..Default::default() };
    assert_eq!(json(&wild, wed), Some(json!({"freq": "daily", "interval": 1, "count": 9999})));
}

#[test]
fn rules_come_back_as_they_were_and_follow_the_start() {
    let tue = day(2026, 10, 13);
    let r = Recurrence { by_day: vec![Weekday::Tue], nth: Some(2), count: Some(12), ..Recurrence::every(Freq::Monthly) };
    assert_eq!(Repeat::of(Some(&r), tue).rule(tue), Some(r.clone()));
    let until = Recurrence { interval: 3, until: Some(day(2027, 1, 1)), by_day: vec![Weekday::Mon, Weekday::Thu], ..Recurrence::every(Freq::Weekly) };
    assert_eq!(Repeat::of(Some(&until), tue).rule(tue), Some(until));
    assert_eq!(Repeat::of(None, tue), Repeat::default());
    // A weekly rule without days is on the start's day, and moves with it.
    let mut weekly = Repeat::of(Some(&Recurrence::every(Freq::Weekly)), tue);
    assert_eq!(weekly.days, [Weekday::Tue]);
    weekly.follow(Some(tue), day(2026, 10, 15));
    assert_eq!(weekly.days, [Weekday::Thu]);
    // Days that were picked stay.
    weekly.days = vec![Weekday::Mon, Weekday::Thu];
    weekly.follow(Some(day(2026, 10, 15)), tue);
    assert_eq!(weekly.days, [Weekday::Mon, Weekday::Thu]);
    // The second Tuesday moved to the last one.
    let mut monthly = Repeat::of(Some(&r), tue);
    monthly.follow(Some(tue), day(2026, 10, 27));
    assert_eq!(monthly.nth, Some(-1));
    monthly.set_freq(Some(Freq::Weekly), Some(tue));
    assert_eq!((monthly.days.as_slice(), monthly.nth, monthly.ends, monthly.count), ([Weekday::Tue].as_slice(), None, Ends::After, 12));
}

#[test]
fn ok_waits_for_the_required_fields() {
    let s = session();
    let mut f = EventForm::new(&s, &json!({}), false);
    assert!(!f.ready());
    f.title = " x ".into();
    assert!(f.ready());
    f.start_time = "soon".into();
    assert!(!f.ready());
    f.all_day = true;
    assert!(f.ready());
    f.end_date = "2026-13-01".into();
    assert!(!f.ready());
    f.end_date = "2026-12-01".into();
    // A new meeting needs someone to invite.
    f.meeting = true;
    assert!(!f.ready());
    f.optional = "a@b.example".into();
    assert!(f.ready());
    // A series that ends on a date needs the date.
    f.repeat = Repeat { freq: Some(Freq::Daily), ends: Ends::Until, ..Default::default() };
    assert!(!f.ready());
    f.repeat.until = "2027-01-01".into();
    assert!(f.ready());
}

#[test]
fn hostile_text_goes_through_without_harm() {
    let mut s = session();
    let junk = "\u{0}\u{202e}é\n<b>".repeat(40_000);
    let p = json!({"title": junk, "location": junk, "description": junk, "attendees": junk, "optional": junk, "start": junk, "end": junk,
        "categories": [junk, 7, null], "reminder": 1e300, "calendar": u64::MAX, "showAs": junk, "allDay": junk});
    let mut f = EventForm::new(&s, &p, false);
    assert_eq!((f.start_date.as_str(), f.reminder, f.calendar), ("2026-10-14", Some(15), s.mailbox.default_calendar().unwrap()));
    for text in [&mut f.start_date, &mut f.start_time, &mut f.end_date, &mut f.end_time, &mut f.repeat.until] {
        text.clone_from(&junk);
    }
    f.start_moved(true);
    f.end_moved();
    f.set_all_day(true, None, false);
    f.set_all_day(false, None, false);
    assert!(!f.ready() && f.length().is_none());
    // Dates that aren't dates go as typed, and the command refuses them.
    let (command, params) = f.command();
    assert!(s.run(command, &params).is_err());
    // With real dates the command takes it, cleaned.
    (f.start_date, f.start_time, f.end_date, f.end_time) = ("2026-10-16".into(), "9".into(), "2026-10-16".into(), "10".into());
    let (command, params) = f.command();
    let refused = s.run(command, &params).unwrap_err().to_string();
    assert!(refused.contains("isn't an e-mail address"));
    assert!(super::super::kit::shown(&refused, 400).chars().count() <= 401);
    f.meeting = false;
    let (command, params) = f.command();
    let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
    let e = s.mailbox.event(id).unwrap();
    assert!(e.title.chars().count() <= 500 && !e.title.contains('\n') && e.duration_minutes() == 60);
}

/// The Appointment Recurrence dialog as it opens on a form.
fn dialog(f: &EventForm) -> RecurrenceDialog {
    RecurrenceDialog::open(&f.repeat, f.series_start(), &f.start_time, &f.end_time, f.minutes, f.all_day)
}

/// A form for half an hour on Tuesday 2026-10-13, the second Tuesday of its month.
fn tuesday(s: &Session) -> EventForm {
    EventForm::new(s, &json!({"start": "2026-10-13T09:30", "end": "2026-10-13T10:00", "title": "Sync"}), false)
}

#[test]
fn the_recurrence_dialog_round_trips_every_kind_of_rule() {
    let s = session();
    let tue = day(2026, 10, 13);
    let rules = vec![
        Recurrence::every(Freq::Daily),
        Recurrence { interval: 3, count: Some(5), ..Recurrence::every(Freq::Daily) },
        Recurrence { by_day: WEEKDAYS.to_vec(), ..Recurrence::every(Freq::Daily) },
        Recurrence { by_day: vec![Weekday::Tue, Weekday::Sat], ..Recurrence::every(Freq::Daily) },
        Recurrence { by_day: vec![Weekday::Tue], ..Recurrence::every(Freq::Weekly) },
        Recurrence {
            interval: 2,
            by_day: vec![Weekday::Mon, Weekday::Tue, Weekday::Fri],
            until: Some(day(2027, 3, 1)),
            ..Recurrence::every(Freq::Weekly)
        },
        Recurrence::every(Freq::Monthly),
        Recurrence { interval: 6, count: Some(12), ..Recurrence::every(Freq::Monthly) },
        Recurrence { by_day: vec![Weekday::Tue], nth: Some(2), ..Recurrence::every(Freq::Monthly) },
        Recurrence::every(Freq::Yearly),
        Recurrence { interval: 4, until: Some(day(2040, 1, 1)), ..Recurrence::every(Freq::Yearly) },
        Recurrence { by_day: vec![Weekday::Tue], nth: Some(2), count: Some(3), ..Recurrence::every(Freq::Yearly) },
    ];
    for rule in rules {
        let mut f = tuesday(&s);
        f.repeat = Repeat::of(Some(&rule), tue);
        assert_eq!(f.repeat.rule(tue), Some(rule.clone()), "{rule:?}");
        let before = f.clone();
        // Opened and left with OK, nothing touched: the dialog says the same rule for
        // the same day, and the form is as it was.
        let d = dialog(&f);
        assert!(!d.touched() && d.had_rule && d.wrong().is_none(), "{rule:?}");
        assert_eq!((d.first_day(tue), d.as_repeat().rule(tue)), (Some(tue), Some(rule.clone())), "{rule:?}");
        f.apply_recurrence(&d, true);
        assert_eq!(f, before, "{rule:?}");
        // Something else chosen for its end: that changes, and the rest of the rule stays.
        let mut d = dialog(&f);
        (d.repeat.ends, d.repeat.count) = (Ends::After, 7);
        assert!(d.touched());
        f.apply_recurrence(&d, true);
        assert_eq!(f.repeat.rule(tue), Some(Recurrence { count: Some(7), until: None, ..rule.clone() }), "{rule:?}");
        assert_eq!(span(&f), span(&before), "{rule:?}");
        // And every way of ending, and none.
        let mut d = dialog(&f);
        (d.repeat.ends, d.repeat.until) = (Ends::Until, "2031-05-06".into());
        f.apply_recurrence(&d, true);
        assert_eq!(f.repeat.rule(tue), Some(Recurrence { count: None, until: Some(day(2031, 5, 6)), ..rule.clone() }), "{rule:?}");
        let mut d = dialog(&f);
        d.repeat.ends = Ends::Never;
        f.apply_recurrence(&d, true);
        assert_eq!(f.repeat.rule(tue), Some(Recurrence { count: None, until: None, ..rule.clone() }), "{rule:?}");
    }
    // The last one of a month, and a fourth one that is also the last, each stay what
    // they were said to be.
    let last = day(2026, 10, 27);
    for nth in [4, -1] {
        let rule = Recurrence { by_day: vec![Weekday::Tue], nth: Some(nth), ..Recurrence::every(Freq::Monthly) };
        let mut f = tuesday(&s);
        f.start_date = last.iso();
        f.start_moved(true);
        f.repeat = Repeat::of(Some(&rule), last);
        let mut d = dialog(&f);
        assert_eq!((d.first_day(last), d.as_repeat().rule(last)), (Some(last), Some(rule.clone())));
        d.repeat.interval = 2;
        f.apply_recurrence(&d, true);
        assert_eq!(f.repeat.rule(last), Some(Recurrence { interval: 2, ..rule }));
    }
}

#[test]
fn a_pattern_on_another_day_starts_the_series_on_the_first_such_day() {
    let s = session();
    let tue = day(2026, 10, 13);
    let mut f = tuesday(&s);
    assert_eq!(f.recurrence_line(true), None);
    // Nothing repeats yet: the dialog offers every week on the item's own day, and OK
    // makes it so.
    let d = dialog(&f);
    assert_eq!((d.freq(), d.repeat.days.as_slice(), d.had_rule, d.touched()), (Freq::Weekly, [Weekday::Tue].as_slice(), false, false));
    f.apply_recurrence(&d, true);
    assert_eq!(f.repeat.rule(tue).map(|r| r.describe(tue)).as_deref(), Some("Every week on Tuesday"));
    assert_eq!(f.recurrence_line(true).as_deref(), Some("Every week on Tuesday, from Oct 13, 2026, 9:30 AM – 10:00 AM"));
    assert_eq!(f.recurrence_line(false).as_deref(), Some("Every week on Tuesday, from Oct 13, 2026, 09:30 – 10:00"));
    // On Thursdays and Fridays instead: the series starts on Thursday the 15th, at the
    // same time of day.
    let mut d = dialog(&f);
    d.repeat.days = vec![Weekday::Fri, Weekday::Thu, Weekday::Fri];
    assert_eq!(d.first_day(tue), Some(day(2026, 10, 15)));
    f.apply_recurrence(&d, true);
    assert_eq!(span(&f), ("2026-10-15", "9:30 AM", "2026-10-15", "10:00 AM"));
    assert_eq!(f.command().1["recurrence"], json!({"freq": "weekly", "interval": 1, "byDay": ["thu", "fri"]}));
    // Monthly: by its date to begin with; the 31st is in the first month that has one.
    let mut d = dialog(&f);
    d.set_freq(Freq::Monthly);
    assert_eq!((d.day, d.repeat.nth, d.first_day(day(2026, 10, 15))), (15, None, Some(day(2026, 10, 15))));
    d.day = 31;
    assert_eq!(d.first_day(day(2026, 10, 15)), Some(day(2026, 10, 31)));
    assert_eq!(d.first_day(day(2026, 11, 1)), Some(day(2026, 12, 31)));
    // The last Friday; and the first Monday, which this month is already past.
    (d.repeat.nth, d.weekday) = (Some(-1), Weekday::Fri);
    assert_eq!(d.first_day(day(2026, 10, 15)), Some(day(2026, 10, 30)));
    (d.repeat.nth, d.weekday) = (Some(1), Weekday::Mon);
    assert_eq!(d.first_day(day(2026, 10, 15)), Some(day(2026, 11, 2)));
    f.apply_recurrence(&d, true);
    assert_eq!(span(&f), ("2026-11-02", "9:30 AM", "2026-11-02", "10:00 AM"));
    assert_eq!(f.command().1["recurrence"], json!({"freq": "monthly", "interval": 1, "byDay": ["mon"], "nth": 1}));
    assert_eq!(f.recurrence_line(true).as_deref(), Some("Every month on the first Monday, from Nov 2, 2026, 9:30 AM – 10:00 AM"));
    // Yearly: February 29th waits for a leap year; the third Thursday of November.
    let mut d = dialog(&f);
    d.set_freq(Freq::Yearly);
    (d.month, d.day) = (2, 29);
    assert_eq!(d.first_day(day(2026, 11, 2)), Some(day(2028, 2, 29)));
    (d.repeat.nth, d.weekday, d.month) = (Some(3), Weekday::Thu, 11);
    assert_eq!(d.first_day(day(2026, 11, 2)), Some(day(2026, 11, 19)));
    f.apply_recurrence(&d, true);
    assert_eq!(f.command().1["recurrence"], json!({"freq": "yearly", "interval": 1, "byDay": ["thu"], "nth": 3}));
    assert_eq!(f.start_date, "2026-11-19");
    // Daily on weekdays, from a Saturday: Monday.
    let mut d = dialog(&f);
    d.set_freq(Freq::Daily);
    d.repeat.days = WEEKDAYS.to_vec();
    assert_eq!((d.first_day(day(2026, 11, 21)), d.first_day(day(2026, 11, 19))), (Some(day(2026, 11, 23)), Some(day(2026, 11, 19))));
    // The time of day is the dialog's too: a new start keeps the length, a new end is it.
    d.start_time = "2pm".into();
    d.start_time_moved(true);
    assert_eq!(d.end_time, "2:30 PM");
    d.end_time = "3:30 PM".into();
    d.end_time_moved();
    f.apply_recurrence(&d, true);
    assert_eq!((span(&f), f.minutes), (("2026-11-19", "2:00 PM", "2026-11-19", "3:30 PM"), 90));
    // An end before the start is on the day after.
    let mut d = dialog(&f);
    (d.start_time, d.end_time) = ("11pm".into(), "1am".into());
    d.end_time_moved();
    assert_eq!(d.minutes, 120);
    // What can't be is said, and changes nothing.
    let before = f.clone();
    let mut d = dialog(&f);
    d.set_freq(Freq::Yearly);
    (d.month, d.day) = (2, 30);
    assert_eq!((d.first_day(tue), d.wrong()), (None, Some("No day fits this pattern")));
    f.apply_recurrence(&d, true);
    assert_eq!(f, before);
    for (start, time, until) in [("soon", "9", "2027-01-01"), ("2026-10-13", "late", "2027-01-01"), ("2026-10-13", "9", "never")] {
        let mut d = dialog(&f);
        (d.start, d.start_time, d.repeat.ends, d.repeat.until) = (start.into(), time.into(), Ends::Until, until.into());
        assert!(d.wrong().is_some(), "{start} {time} {until}");
    }
    // A whole day has no time of day to it.
    let mut f = EventForm::new(&s, &json!({"start": "2026-10-13", "allDay": true, "title": "Fair"}), false);
    let mut d = dialog(&f);
    d.start_time = "nonsense".into();
    assert!(d.wrong().is_none());
    d.repeat.days = vec![Weekday::Sat];
    f.apply_recurrence(&d, true);
    assert_eq!((f.start_date.as_str(), f.end_date.as_str(), f.all_day), ("2026-10-17", "2026-10-17", true));
    assert_eq!(f.recurrence_line(true).as_deref(), Some("Every week on Saturday, from Oct 17, 2026"));
    // Numbers and text from nowhere do no harm.
    let junk = "\u{0}\u{202e}é\n".repeat(10_000);
    let mut d = dialog(&f);
    (d.day, d.month, d.repeat.interval, d.repeat.count, d.repeat.nth) = (u32::MAX, u8::MAX, u32::MAX, u32::MAX, Some(i8::MIN));
    (d.start, d.start_time, d.end_time, d.repeat.until) = (junk.clone(), junk.clone(), junk.clone(), junk);
    d.start_time_moved(true);
    d.end_time_moved();
    for freq in [Freq::Daily, Freq::Weekly, Freq::Monthly, Freq::Yearly] {
        d.repeat.freq = Some(freq);
        let _ = (d.first_day(day(9999, 12, 31)), d.first_day(day(1, 1, 1)), d.wrong(), d.as_repeat());
        f.apply_recurrence(&d, false);
    }
}

#[test]
fn names_on_the_attendee_lines_are_checked_as_the_composer_checks_them() {
    let s = session();
    // A name that is one contact's becomes that contact; an address stays as typed.
    let (line, unresolved) = check_names(&s, "maya; someone@else.example;  Tomás Rivera <tomas@lumen.example>");
    assert_eq!(line, "Maya Okafor <maya@lumen.example>; someone@else.example; Tomás Rivera <tomas@lumen.example>");
    assert!(unresolved.is_empty());
    // Nobody's name, and a name several have, stay and are said.
    let (line, unresolved) = check_names(&s, "zzzz; lumen");
    assert_eq!((line.as_str(), unresolved), ("zzzz; lumen", vec!["zzzz".to_string(), "lumen".to_string()]));
    assert_eq!(check_names(&s, " ; ;"), (String::new(), Vec::new()));
    // On the form: both lines, and only what changes is changed.
    let mut f = EventForm::new(&s, &json!({"attendees": "priya@lumen.example;maya", "optional": "nobody at all"}), true);
    assert_eq!(f.check_names(&s), ["nobody at all"]);
    assert_eq!((f.required.as_str(), f.optional.as_str()), ("priya@lumen.example; Maya Okafor <maya@lumen.example>", "nobody at all"));
    // What a line shows of the people on it: a name each, the contact's when the line
    // has only the address; what is nobody stands out.
    let t = crate::theme::Tokens::light();
    let job = names_job(&s, "priya@lumen.example; Maya Okafor <maya@lumen.example>; who@nowhere.example; bob", egui::FontId::default(), &t);
    assert_eq!(job.text, "Priya Nair; Maya Okafor; who@nowhere.example; bob");
    let colors: Vec<egui::Color32> = job.sections.iter().map(|section| section.format.color).collect();
    assert_eq!(colors.last(), Some(&t.red));
    assert!(colors.iter().filter(|c| **c == t.red).count() == 1);
    // Hostile lines are cut to size.
    let junk = "\u{0}\u{202e}é<b>;".repeat(20_000);
    let (line, unresolved) = check_names(&s, &junk);
    assert!(unresolved.len() <= 500 && unresolved.iter().all(|name| name.chars().count() <= 80) && !line.is_empty());
    assert!(names_job(&s, &junk, egui::FontId::default(), &t).text.chars().count() < 200_000);
}

#[test]
fn one_occurrence_shows_on_its_own_day_and_is_saved_as_that_one() {
    let mut s = session();
    let standup = event_id(&s, "Team stand-up");
    let series = (**s.mailbox.events.get(&standup).unwrap()).clone();
    let friday = day(2026, 10, 16);
    // Opened as the one on Friday: the form shows Friday, and the command says which.
    let f = EventForm::edit(&s, &json!({"id": standup, "scope": "occurrence", "occurrence": "2026-10-16"})).unwrap();
    assert_eq!((f.scope, f.start_date.as_str(), f.end_date.as_str()), (Some(Scope::One(friday)), "2026-10-16", "2026-10-16"));
    assert_eq!(f.start_time, series.start.time_short(true));
    assert!(!f.dirty());
    assert_eq!(f.command(), ("calendar.edit", json!({"id": standup, "scope": "occurrence", "occurrence": "2026-10-16"})));
    // It has no rule of its own to change, and its bar has no Recurrence.
    let mut f = f;
    f.open_recurrence();
    assert!(f.recurrence.is_none() && !f.bar_items().iter().any(|item| item.key == "recurrence"));
    assert!(f.recurrence_line(true).is_some());
    // Another time that day, the title left alone: the command is told the change as
    // one to the series' first day, which is how one occurrence is changed, and that
    // occurrence becomes an event of its own on its own day.
    f.start_time = "11am".into();
    f.start_moved(true);
    let (command, params) = f.command();
    assert_eq!(
        (params["start"].as_str(), params["occurrence"].as_str(), params.get("title"), params.get("recurrence")),
        (Some(format!("{}T11:00", series.start.date.iso()).as_str()), Some("2026-10-16"), None, None)
    );
    let made = s.run(command, &params).unwrap();
    let one = (**s.mailbox.events.get(&made["id"].as_u64().unwrap()).unwrap()).clone();
    assert_eq!((one.start.date, one.start.hour, one.duration_minutes(), one.recurrence.is_none()), (friday, 11, series.duration_minutes(), true));
    let after = (**s.mailbox.events.get(&standup).unwrap()).clone();
    assert_eq!((after.start, after.recurrence.clone(), after.exceptions.contains(&friday)), (series.start, series.recurrence.clone(), true));
    // Moved to another day: by as many days.
    let mut f = EventForm::edit(&s, &json!({"id": standup, "scope": "occurrence", "occurrence": "2026-10-20"})).unwrap();
    let rule = f.repeat.clone();
    f.start_date = "2026-10-24".into();
    f.start_moved(true);
    assert_eq!(f.repeat, rule, "the series' rule is not the occurrence's to move");
    let (command, params) = f.command();
    let made = s.run(command, &params).unwrap();
    assert_eq!(s.mailbox.events.get(&made["id"].as_u64().unwrap()).unwrap().start.date, day(2026, 10, 24));
    // Deleting it deletes that one.
    let f = EventForm::edit(&s, &json!({"id": standup, "scope": "occurrence", "occurrence": "2026-10-21"})).unwrap();
    assert_eq!(f.scoped(json!({"id": standup})), json!({"id": standup, "scope": "occurrence", "occurrence": "2026-10-21"}));
    // The series: said so, and it shows its own first day.
    let f = EventForm::edit(&s, &json!({"id": standup, "scope": "series"})).unwrap();
    assert_eq!((f.scope, f.start_date.as_str(), f.apart), (Some(Scope::Series), series.start.date.iso().as_str(), 0));
    assert_eq!(f.command().1, json!({"id": standup, "scope": "series"}));
    assert!(f.bar_items().iter().any(|item| item.key == "recurrence"));
    // Nothing said, a day the series doesn't happen on, an item that doesn't repeat, and
    // nonsense: nothing is assumed, and the command says nothing of it.
    let dentist = event_id(&s, "Dentist");
    for (id, p) in [
        (standup, json!({"id": standup})),
        (standup, json!({"id": standup, "scope": "occurrence", "occurrence": "2026-10-17"})),
        (standup, json!({"id": standup, "scope": "occurrence", "occurrence": "never"})),
        (standup, json!({"id": standup, "scope": 7, "occurrence": [1]})),
        (standup, json!({"id": standup, "scope": "everything"})),
        (dentist, json!({"id": dentist, "scope": "occurrence", "occurrence": "2026-10-17"})),
        (dentist, json!({"id": dentist, "scope": "series"})),
    ] {
        let f = EventForm::edit(&s, &p).unwrap();
        assert_eq!((f.scope, f.apart, f.command().1), (None, 0, json!({"id": id})), "{p}");
    }
}
