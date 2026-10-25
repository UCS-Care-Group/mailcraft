// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Other people's calendars in the stand-in (#70, Open Shared Calendar): what
//! `/v1.0/users/{address}/calendar`, `/calendar/calendarView`, `/calendar/events[/{id}]` and
//! `/users/{address}/events[/{id}]` answer when the person shared their calendar with the owner
//! of the token, and `POST /me/calendar/getSchedule` for the time of people who share only when
//! they are free and busy.
//!
//! A person has a calendar of their own, answered as the owner's is (their events are found
//! through it only), and an [`Access`] that says what the owner of the token may do with it.
//! Nothing of theirs is ever answered at `/me`: a test looks at [`State::requests`].

use mailcraft_time::Timestamp;
use serde_json::{Value, json};

use super::{Answer, Asked, Calendar, Refusal, State, bad, refuse, stamp};

/// What the owner of the token may do with a person's calendar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Access {
    /// Nothing is shared: 403 `ErrorAccessDenied` for the calendar and everything in it.
    #[default]
    None,
    /// Free and busy only: the calendar can't be read (403), the schedule can.
    FreeBusy,
    /// The calendar and its events can be read.
    Read,
    /// And changed.
    Edit,
    /// There is no mailbox at the address (404 `ErrorInvalidUser`).
    Missing,
}

/// A person whose calendar the stand-in knows.
#[derive(Clone, Debug, Default)]
pub struct Person {
    pub address: String,
    pub name: String,
    pub access: Access,
    calendars: Vec<Calendar>,
    events: Vec<super::Stored>,
    /// When their time is taken, for the schedule: start, end and Graph's name for how it shows.
    pub busy: Vec<(Timestamp, Timestamp, String)>,
}

impl State {
    /// A person at `address` called `name`, whose calendar is shared as `access` says: the id of
    /// their calendar.
    pub fn add_person(&mut self, address: &str, name: &str, access: Access) -> String {
        let id = {
            self.made += 1;
            format!("AAMkAGVmMDEzMTM4LTZmYWUtNDdkNC1hMDZiLTU1OGY5OTZhYmY4OABGAAAAAAAiQ8W967B7TKBjgx9rVEURBwAiIsqMbYjsT5e-person{:04}AAA=", self.made)
        };
        let calendar =
            Calendar { id: id.clone(), name: "Calendar".into(), main: true, can_edit: access == Access::Edit, shared: true, online: Vec::new() };
        self.people.push(Person { address: address.into(), name: name.into(), access, calendars: vec![calendar], ..Default::default() });
        id
    }

    /// The person at `address`, to set up and to look at.
    pub fn person(&mut self, address: &str) -> Option<&mut Person> {
        self.people.iter_mut().find(|person| person.address.eq_ignore_ascii_case(address))
    }

    /// An event in the calendar of the person at `address`, an hour long: its id.
    pub fn add_person_event(&mut self, address: &str, subject: &str, start: Timestamp) -> String {
        let Some(at) = self.people.iter().position(|person| person.address.eq_ignore_ascii_case(address)) else { return String::new() };
        let main = self.people.get(at).and_then(|person| person.calendars.first()).map(|calendar| calendar.id.clone()).unwrap_or_default();
        self.swap_person(at);
        let id = self.add_event(&main, subject, start);
        self.swap_person(at);
        id
    }

    /// Put the calendars and events of the person `at` in place of the owner's, or back.
    fn swap_person(&mut self, at: usize) {
        if let Some(person) = self.people.get_mut(at) {
            std::mem::swap(&mut self.calendars, &mut person.calendars);
            std::mem::swap(&mut self.events, &mut person.events);
        }
    }

    /// `/v1.0/users/{address}/…` when the address is a person's: their calendar, as the owner's is.
    pub(super) fn of_person(&mut self, asked: &Asked, method: &str, at: usize, rest: &[&str]) -> Result<Answer, Refusal> {
        let Some(person) = self.people.get(at) else { return Err(super::not_found()) };
        let denied = || refuse(403, "ErrorAccessDenied", "Access is denied. Check credentials and try again.");
        let (access, name, address) = (person.access, person.name.clone(), person.address.clone());
        // `POST /users/{address}/calendar/getSchedule`: free and busy times are what a person who
        // shares nothing else may still be asked for.
        if rest == ["calendar", "getSchedule"] && method == "POST" && access != Access::Missing {
            return self.get_schedule(asked);
        }
        match access {
            Access::Missing => return Err(refuse(404, "ErrorInvalidUser", format!("The requested user '{address}' is invalid."))),
            Access::None | Access::FreeBusy => return Err(denied()),
            Access::Read | Access::Edit => {}
        }
        let main = self.people.get(at).and_then(|person| person.calendars.first()).map(|calendar| calendar.id.clone()).unwrap_or_default();
        if rest == ["calendar"] && method == "GET" {
            let can_edit = access == Access::Edit;
            let body = json!({
                "id": main, "name": "Calendar", "color": "auto", "hexColor": "", "canEdit": can_edit, "canShare": false,
                "canViewPrivateItems": false, "isDefaultCalendar": true,
                "owner": {"name": name, "address": address},
            });
            return Ok((200, Some(body)));
        }
        self.swap_person(at);
        let owner = std::mem::replace(&mut self.owner, address.clone());
        let before = std::mem::replace(&mut self.at, format!("users/{}", crate::http::encode(&address).replace("%40", "@")));
        let answer = match rest {
            ["calendar", "calendarView"] | ["calendarView"] if method == "GET" => self.calendar_view(asked, None),
            ["calendar", "events", tail @ ..] | ["events", tail @ ..] => self.events_at(asked, method, Some(main.as_str()), tail),
            _ => Err(refuse(400, "BadRequest", "Resource not found for the segment.")),
        };
        self.at = before;
        self.owner = owner;
        self.swap_person(at);
        answer
    }

    /// `POST /me/calendar/getSchedule`: when the people asked about are busy.
    pub(super) fn get_schedule(&mut self, asked: &Asked) -> Result<Answer, Refusal> {
        let body = asked.json();
        let people: Vec<String> = body
            .get("schedules")
            .and_then(Value::as_array)
            .map(|all| all.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        if people.is_empty() {
            return Err(bad("The schedules to ask about are missing."));
        }
        let entries: Vec<Value> = people
            .iter()
            .map(|asked| match self.people.iter().find(|person| person.address.eq_ignore_ascii_case(asked)) {
                Some(person) if matches!(person.access, Access::FreeBusy | Access::Read | Access::Edit) => {
                    let items: Vec<Value> = person
                        .busy
                        .iter()
                        .map(|(start, end, status)| {
                            json!({"status": status, "start": {"dateTime": stamp(*start), "timeZone": "UTC"}, "end": {"dateTime": stamp(*end), "timeZone": "UTC"}})
                        })
                        .collect();
                    json!({"scheduleId": asked, "availabilityView": "0", "scheduleItems": items})
                }
                _ => json!({"scheduleId": asked, "availabilityView": "", "scheduleItems": [],
                    "error": {"message": "We couldn't find the mailbox.", "responseCode": "ErrorMailRecipientNotFound"}}),
            })
            .collect();
        Ok((200, Some(json!({"value": entries}))))
    }
}
