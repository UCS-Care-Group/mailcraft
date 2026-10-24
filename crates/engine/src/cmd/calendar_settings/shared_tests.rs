// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #70: whose a calendar is and what the account may do with it: the folder pane's Shared
//! Calendars, and the commands that refuse a change the rights don't allow.

use mailcraft_model::remote::{CalendarsDownload, RemoteCalendar};
use mailcraft_model::{CalendarOwner, Id, Rights};
use serde_json::json;

use super::*;

const NOW: i64 = 1_791_993_600;

fn served() -> (Session, Id) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let added = s.run("account.add", &json!({"email": "ada@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"})).unwrap();
    (s, added["id"].as_u64().unwrap())
}

fn owned(address: &str, name: &str) -> Option<CalendarOwner> {
    Some(CalendarOwner { address: address.into(), name: name.into() })
}

fn entry(id: &str, name: &str, group: &str, rights: Rights, owner: Option<CalendarOwner>) -> RemoteCalendar {
    RemoteCalendar { id: id.into(), name: name.into(), group: group.into(), rights, owner, ..Default::default() }
}

fn listed(s: &mut Session, account: Id) {
    let all = vec![
        RemoteCalendar { main: true, ..entry("c-main", "Calendar", "My Calendars", Rights::Edit, owned("ada@example.org", "Ada")) },
        entry("c-team", "Team", "Projects", Rights::Edit, owned("ADA@example.org", "Ada")),
        entry("c-bo", "Bo", "Other Calendars", Rights::Read, owned("bo@example.org", "Bo Example")),
        entry("c-cy", "Cy", "My Calendars", Rights::Edit, owned("cy@example.org", "Cy")),
        entry("c-hol", "Holidays", "Other Calendars", Rights::Read, None),
    ];
    let download = CalendarsDownload { account, email: "ada@example.org".into(), complete: true, calendars: all, ..Default::default() };
    remote::apply_calendars(s, &download).unwrap();
}

fn titles(s: &Session) -> Vec<(String, Vec<String>)> {
    let name = |id: &Id| s.mailbox.calendar(*id).map(|c| c.name.clone()).unwrap_or_default();
    calendar_groups(s).iter().filter(|g| g.account != 0).map(|g| (g.title.clone(), g.calendars.iter().map(name).collect())).collect()
}

#[test]
fn what_other_people_share_is_listed_under_shared_calendars() {
    let (mut s, account) = served();
    listed(&mut s, account);
    let by = |title: &str| titles(&s).into_iter().find(|(t, _)| t == title).map(|(_, names)| names);
    assert_eq!(by("My Calendars"), Some(vec!["Calendar".to_string()]));
    // Whatever group the server puts them in; the owner's own address, in any case, is not "shared".
    assert_eq!(by("Shared Calendars"), Some(vec!["Bo".to_string(), "Cy".to_string()]));
    assert_eq!(by("Projects"), Some(vec!["Team".to_string()]));
    // One whose owner the server didn't say keeps the server's group.
    assert_eq!(by("Other Calendars"), Some(vec!["Holidays".to_string()]));
    let order: Vec<String> = titles(&s).into_iter().map(|(t, _)| t).collect();
    assert_eq!(order, ["My Calendars", "Shared Calendars", "Projects", "Other Calendars"]);
    // The list says whose and what rights.
    let answer = s.run("calendar.calendars", &json!({})).unwrap();
    let text = answer.to_string();
    assert!(text.contains("\"rights\":\"read\"") && text.contains("bo@example.org"), "{text}");
}

#[test]
fn rights_and_owner_follow_the_server_at_every_listing() {
    let (mut s, account) = served();
    listed(&mut s, account);
    let bo = |s: &Session| s.mailbox.calendars.iter().find(|c| c.name == "Bo").cloned().unwrap();
    assert_eq!((bo(&s).rights, bo(&s).owner), (Rights::Read, owned("bo@example.org", "Bo Example")));
    let again = CalendarsDownload {
        account,
        email: "ada@example.org".into(),
        complete: false,
        calendars: vec![
            RemoteCalendar { main: true, ..entry("c-main", "Calendar", "My Calendars", Rights::Edit, None) },
            entry("c-bo", "Bo", "Other Calendars", Rights::Edit, owned("bo@example.org", "Bo E.")),
        ],
        ..Default::default()
    };
    remote::apply_calendars(&mut s, &again).unwrap();
    assert_eq!((bo(&s).rights, bo(&s).owner), (Rights::Edit, owned("bo@example.org", "Bo E.")));
}

#[test]
fn commands_refuse_a_change_the_rights_do_not_allow() {
    let (mut s, account) = served();
    listed(&mut s, account);
    let id = |s: &Session, name: &str| s.mailbox.calendars.iter().find(|c| c.name == name).map(|c| c.id).unwrap();
    let (team, bo) = (id(&s, "Team"), id(&s, "Bo"));
    let made = s.run("calendar.new", &json!({"title": "Retro", "start": "2026-10-16T10:00", "calendar": team})).unwrap()["id"].as_u64().unwrap();
    // Read: nothing goes in, and what is in can't be changed or deleted.
    let why = s.run("calendar.new", &json!({"title": "Nope", "start": "2026-10-16T11:00", "calendar": bo})).unwrap_err().to_string();
    assert!(why.contains("can only be read"), "{why}");
    for (rights, word) in [(Rights::Read, "can only be read"), (Rights::FreeBusy, "only shows when its owner is busy")] {
        s.mailbox.calendars.iter_mut().find(|c| c.id == team).unwrap().rights = rights;
        for (command, params) in [
            ("calendar.move", json!({"id": made, "start": "2026-10-16T12:00"})),
            ("calendar.delete", json!({"id": made})),
            ("calendar.new", json!({"title": "More", "start": "2026-10-16T13:00", "calendar": team})),
        ] {
            let why = s.run(command, &params).unwrap_err().to_string();
            assert!(why.contains(word), "{command}: {why}");
        }
        assert!(s.mailbox.event(made).is_some());
    }
    // Edit again: it works.
    s.mailbox.calendars.iter_mut().find(|c| c.id == team).unwrap().rights = Rights::Edit;
    assert!(s.run("calendar.move", &json!({"id": made, "start": "2026-10-16T12:00"})).is_ok());
}
