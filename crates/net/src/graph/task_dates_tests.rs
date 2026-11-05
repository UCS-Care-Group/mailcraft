// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The start and due dates of a Microsoft To Do task (#809): a day, which Graph writes as the
//! midnight that begins it, either in the zone it names or turned into UTC. Read either way, it is
//! the same day here, east or west of UTC and on either side of a change of the clocks; and a day
//! sent from here is the owner's midnight, so it is the same day there.

use mailcraft_model::TaskPatch;
use mailcraft_time::{Date, Zone};
use serde_json::{Value, json};

use super::tasks::{task_body, task_of};

fn zone(name: &str) -> Zone {
    Zone::named(name).unwrap()
}

fn day(year: i32, month: u8, day: u8) -> Date {
    Date::new(year, month, day).unwrap()
}

/// The due and start dates of a task whose dates Graph wrote as `due` and `start`, read for an
/// owner in `owner`.
fn read(due: Value, start: Value, owner: Zone) -> (Option<Date>, Option<Date>) {
    let entry = json!({"id": "t1", "title": "x", "dueDateTime": due, "startDateTime": start});
    let task = task_of(&entry, "L1", owner).unwrap();
    (task.due, task.start)
}

fn utc(when: &str) -> Value {
    json!({"dateTime": when, "timeZone": "UTC"})
}

#[test]
fn a_day_graph_gives_as_the_owners_midnight_in_utc_is_that_day_east_of_utc() {
    // The report: 10 November in Microsoft To Do, Graph's 23:00 UTC the day before, and it
    // showed as 9 November here.
    let berlin = zone("Europe/Berlin");
    let (due, start) = read(utc("2026-11-09T23:00:00.0000000"), utc("2026-11-09T23:00:00.0000000"), berlin);
    assert_eq!((due, start), (Some(day(2026, 11, 10)), Some(day(2026, 11, 10))));
    // An offset that is only an offset says the same.
    assert_eq!(read(utc("2026-11-09T23:00:00.0000000"), Value::Null, Zone::fixed(60).unwrap()).0, Some(day(2026, 11, 10)));
}

#[test]
fn a_day_graph_gives_as_the_owners_midnight_in_utc_is_that_day_west_of_utc() {
    let new_york = zone("America/New_York");
    let (due, start) = read(utc("2026-11-10T05:00:00.0000000"), utc("2026-11-09T05:00:00.0000000"), new_york);
    assert_eq!((due, start), (Some(day(2026, 11, 10)), Some(day(2026, 11, 9))));
    assert_eq!(read(utc("2026-11-10T05:00:00.0000000"), Value::Null, Zone::fixed(-5 * 60).unwrap()).0, Some(day(2026, 11, 10)));
}

#[test]
fn a_day_on_either_side_of_a_change_of_the_clocks_stays_on_its_day() {
    // Summer time ends in Europe on 25 October 2026 and in the United States on 1 November 2026:
    // the midnight before is an hour further from UTC than the one after.
    let berlin = zone("Europe/Berlin");
    assert_eq!(read(utc("2026-10-24T22:00:00.0000000"), Value::Null, berlin).0, Some(day(2026, 10, 25)));
    assert_eq!(read(utc("2026-10-25T23:00:00.0000000"), Value::Null, berlin).0, Some(day(2026, 10, 26)));
    assert_eq!(read(utc("2026-03-28T23:00:00.0000000"), Value::Null, berlin).0, Some(day(2026, 3, 29)));
    assert_eq!(read(utc("2026-03-29T22:00:00.0000000"), Value::Null, berlin).0, Some(day(2026, 3, 30)));
    let new_york = zone("America/New_York");
    assert_eq!(read(utc("2026-11-01T04:00:00.0000000"), Value::Null, new_york).0, Some(day(2026, 11, 1)));
    assert_eq!(read(utc("2026-11-02T05:00:00.0000000"), Value::Null, new_york).0, Some(day(2026, 11, 2)));
}

#[test]
fn a_midnight_written_in_its_own_zone_is_its_day_wherever_the_owner_is() {
    for owner in [zone("Europe/Berlin"), zone("America/New_York"), Zone::UTC, Zone::LOCAL] {
        // A midnight in UTC (as this program wrote dates before #809), in London in summer and in
        // a zone Windows names.
        assert_eq!(read(utc("2026-10-30T00:00:00.0000000"), Value::Null, owner).0, Some(day(2026, 10, 30)));
        let london = json!({"dateTime": "2026-07-14T00:00:00.0000000", "timeZone": "Europe/London"});
        assert_eq!(read(london, Value::Null, owner).0, Some(day(2026, 7, 14)));
        let windows = json!({"dateTime": "2026-11-10T00:00:00.0000000", "timeZone": "W. Europe Standard Time"});
        assert_eq!(read(windows, Value::Null, owner).0, Some(day(2026, 11, 10)));
    }
}

#[test]
fn a_day_from_a_zone_a_little_off_the_owners_is_still_its_day() {
    // To Do's zone one hour east of the owner's, or one hour west: the nearest midnight.
    let owner = Zone::fixed(0).unwrap();
    assert_eq!(read(utc("2026-11-09T23:00:00.0000000"), Value::Null, owner).0, Some(day(2026, 11, 10)));
    assert_eq!(read(utc("2026-11-10T01:00:00.0000000"), Value::Null, owner).0, Some(day(2026, 11, 10)));
}

#[test]
fn a_date_that_cant_be_placed_is_read_as_written_and_nonsense_is_no_date() {
    let owner = zone("Europe/Berlin");
    // A zone nobody knows: the day as written, as before.
    let unknown = json!({"dateTime": "2026-11-09T23:00:00.0000000", "timeZone": "Mars/Olympus"});
    assert_eq!(read(unknown, Value::Null, owner).0, Some(day(2026, 11, 9)));
    for nonsense in [json!({"dateTime": "not a date"}), json!({"dateTime": 7}), json!("2026-11-10"), json!({"dateTime": "2026-13-40T23:00:00"})] {
        assert_eq!(read(nonsense, Value::Null, owner).0, None);
    }
    let long = json!({"dateTime": format!("2026-11-09T23:00:00{}", "0".repeat(10_000)), "timeZone": "UTC"});
    assert_eq!(read(long, Value::Null, owner).0, Some(day(2026, 11, 9)));
}

#[test]
fn a_time_with_its_own_offset_is_the_instant_it_names() {
    let berlin = zone("Europe/Berlin");
    let windows = |when: &str| json!({"dateTime": when, "timeZone": "W. Europe Standard Time"});
    assert_eq!(read(json!({"dateTime": "2026-11-09T23:00:00Z"}), Value::Null, berlin).0, Some(day(2026, 11, 10)));
    assert_eq!(read(json!({"dateTime": "2026-11-10T00:00:00+01:00"}), Value::Null, berlin).0, Some(day(2026, 11, 10)));
    assert_eq!(read(json!({"dateTime": "2026-11-10T00:00:00Z"}), Value::Null, zone("America/New_York")).0, Some(day(2026, 11, 10)));
    // A wall time in a zone Windows names, not midnight: that instant, on the owner's day.
    assert_eq!(read(windows("2026-11-09T23:00:00.0000000"), Value::Null, zone("Europe/London")).0, Some(day(2026, 11, 10)));
}

#[test]
fn a_midnight_the_clocks_skip_is_sent_as_its_instant() {
    // Chile's clocks go from 00:00 to 01:00 on 6 September 2026: that day has no midnight there.
    let santiago = zone("America/Santiago");
    let body = task_body(&TaskPatch { due: Some(Date::new(2026, 9, 6)), ..Default::default() }, santiago);
    assert_eq!(body["dueDateTime"]["timeZone"], "UTC");
    assert_eq!(read(body["dueDateTime"].clone(), Value::Null, santiago).0, Some(day(2026, 9, 6)));
}

#[test]
fn a_day_is_sent_as_the_owners_midnight_named_by_its_zone() {
    let patch = TaskPatch { start: Some(Date::new(2026, 11, 10)), due: Some(Date::new(2026, 11, 10)), ..Default::default() };
    let body = task_body(&patch, zone("Europe/Berlin"));
    let midnight = json!({"dateTime": "2026-11-10T00:00:00.0000000", "timeZone": "W. Europe Standard Time"});
    assert_eq!((&body["dueDateTime"], &body["startDateTime"]), (&midnight, &midnight));
    let body = task_body(&patch, zone("America/New_York"));
    assert_eq!(body["dueDateTime"], json!({"dateTime": "2026-11-10T00:00:00.0000000", "timeZone": "Eastern Standard Time"}));
    // A zone Graph has no name for (an offset, or the system's without a name): its midnight in UTC.
    let body = task_body(&patch, Zone::fixed(-5 * 60).unwrap());
    assert_eq!(body["dueDateTime"], json!({"dateTime": "2026-11-10T05:00:00.0000000", "timeZone": "UTC"}));
    let body = task_body(&patch, Zone::UTC);
    assert_eq!(body["dueDateTime"], json!({"dateTime": "2026-11-10T00:00:00.0000000", "timeZone": "UTC"}));
    // Taking a date away is still `null`.
    let body = task_body(&TaskPatch { due: Some(None), ..Default::default() }, zone("Europe/Berlin"));
    assert_eq!(body, json!({"dueDateTime": null}));
}

/// What Graph gives back for a date sent as `sent`: the same midnight, turned into UTC.
fn as_graph_gives_it(sent: &Value) -> Value {
    let wall = mailcraft_time::parse_iso(sent["dateTime"].as_str().unwrap()).unwrap().0;
    let at = Zone::named(sent["timeZone"].as_str().unwrap()).unwrap().instant(wall);
    utc(&format!("{}.0000000", mailcraft_time::DateTime::from_timestamp(at, 0).iso()))
}

#[test]
fn a_day_sent_and_read_back_is_the_same_day_east_and_west_of_utc() {
    let days = [day(2026, 11, 10), day(2026, 10, 25), day(2026, 10, 26), day(2026, 3, 29), day(2026, 11, 1), day(2026, 7, 14), day(2026, 9, 6)];
    let owners = [
        zone("Europe/Berlin"),
        zone("America/New_York"),
        zone("Asia/Kolkata"),
        zone("Asia/Kathmandu"),
        zone("Pacific/Auckland"),
        zone("Pacific/Tongatapu"),
        zone("Pacific/Kiritimati"),
        zone("Pacific/Honolulu"),
        zone("Etc/GMT+12"),
        zone("America/Santiago"),
        zone("Atlantic/Azores"),
        Zone::fixed(60).unwrap(),
        Zone::fixed(-5 * 60).unwrap(),
        Zone::UTC,
        Zone::LOCAL,
    ];
    for owner in owners {
        for want in days {
            let body = task_body(&TaskPatch { due: Some(Some(want)), ..Default::default() }, owner);
            let sent = &body["dueDateTime"];
            // Back as it was sent, and back as Graph turns it into UTC.
            assert_eq!(read(sent.clone(), Value::Null, owner).0, Some(want), "{owner:?} {want:?} as sent");
            assert_eq!(read(as_graph_gives_it(sent), Value::Null, owner).0, Some(want), "{owner:?} {want:?} in UTC");
        }
    }
}
