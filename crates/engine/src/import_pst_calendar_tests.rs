// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendar items and tasks of a .pst file (#815): synthetic files by the reader's test writer,
//! invented people at `example.org`. Nothing is sent for any of them.

use mailcraft_model::{Freq, Importance, Mailbox, Response, ShowAs};
use mailcraft_pst::test_writer::{self as w, FileSpec, FolderSpec, Layout, MessageSpec, Value, recurrence_bytes, time_zone_bytes};
use mailcraft_pst::{Encryption, Format, PS_PUBLIC_STRINGS};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value as Json, json};

use crate::Session;
use crate::import_pst::calendar::{PSETID_APPOINTMENT, PSETID_COMMON, PSETID_MEETING};
use crate::import_pst::item::PSETID_TASK;
use crate::import_pst::run_staged;
use crate::import_pst_memory_tests::{Temp, kept_session};

/// The named properties the writer is given, in the order of their ids (0x8000 on).
const NAMED: [([u8; 16], u32); 17] = [
    (PSETID_APPOINTMENT, 0x8205),
    (PSETID_APPOINTMENT, 0x8208),
    (PSETID_APPOINTMENT, 0x820D),
    (PSETID_APPOINTMENT, 0x820E),
    (PSETID_APPOINTMENT, 0x8215),
    (PSETID_APPOINTMENT, 0x8216),
    (PSETID_APPOINTMENT, 0x8217),
    (PSETID_APPOINTMENT, 0x8223),
    (PSETID_APPOINTMENT, 0x825E),
    (PSETID_APPOINTMENT, 0x8260),
    (PSETID_COMMON, 0x8501),
    (PSETID_COMMON, 0x8503),
    (PSETID_MEETING, 0x0003),
    (PSETID_TASK, 0x8104),
    (PSETID_TASK, 0x8105),
    (PSETID_TASK, 0x810F),
    (PSETID_TASK, 0x811C),
];

fn lid(l: u32) -> u16 {
    0x8000 + NAMED.iter().position(|x| x.1 == l).unwrap() as u16
}

fn at(y: i32, m: u8, d: u8, h: u8, min: u8) -> i64 {
    DateTime::new(Date::new(y, m, d).unwrap(), h, min, 0).unwrap().to_timestamp(0)
}

fn filetime(ts: i64) -> u64 {
    ((ts + 11_644_473_600) * 10_000_000) as u64
}

/// Minutes since 1601 of a wall-clock time.
fn minutes(ts: i64) -> u32 {
    (ts / 60 + 194_074_560) as u32
}

fn folder(name: &str, class: &str, messages: Vec<MessageSpec>) -> FolderSpec {
    FolderSpec { name: name.into(), class: Some(class.into()), messages, children: vec![] }
}

fn file(folders: Vec<FolderSpec>) -> Vec<u8> {
    w::build(&FileSpec {
        layout: Layout::new(Format::Unicode, Encryption::Permute),
        password: None,
        password_ansi: false,
        name: "Personal Folders".into(),
        folders,
        named: NAMED.to_vec(),
        named_strings: vec![(PS_PUBLIC_STRINGS, "Keywords".into())],
    })
}

fn appointment(subject: &str, start: i64, end: i64, extra: Vec<(u16, Value)>) -> MessageSpec {
    let mut props = vec![(lid(0x820D), w::time(filetime(start))), (lid(0x820E), w::time(filetime(end)))];
    props.extend(extra);
    MessageSpec { class: "IPM.Appointment".into(), subject: subject.into(), body: format!("Notes of {subject}"), extra: props, ..Default::default() }
}

fn task(subject: &str, extra: Vec<(u16, Value)>) -> MessageSpec {
    MessageSpec { class: "IPM.Task".into(), subject: subject.into(), body: "Task notes".into(), extra, ..Default::default() }
}

fn run(s: &mut Session, dir: &Temp, bytes: &[u8], v: Json) -> Json {
    let pst = dir.pst(bytes);
    run_staged(s, &pst, None, false, &v).unwrap()
}

fn event<'a>(s: &'a Session, title: &str) -> &'a mailcraft_model::Event {
    s.mailbox.events.values().find(|e| e.title == title).unwrap()
}

fn adds_up(answer: &Json) {
    for r in answer["folders"].as_array().unwrap() {
        let sum: u64 = ["added", "already", "leftOut", "later"].iter().map(|k| r[*k].as_u64().unwrap()).sum();
        assert_eq!(sum, r["found"].as_u64().unwrap(), "{r}");
    }
}

fn notes(answer: &Json) -> Vec<String> {
    answer["notes"].as_array().unwrap().iter().map(|n| n["why"].as_str().unwrap().to_string()).collect()
}

#[test]
fn an_appointment_keeps_its_fields_in_a_calendar_of_its_own() {
    let dir = Temp::new("cal-fields");
    let mut s = kept_session(&dir);
    let a = appointment(
        "Dentist",
        at(2026, 3, 2, 9, 0),
        at(2026, 3, 2, 9, 30),
        vec![
            (lid(0x8208), w::unicode("High Street")),
            (lid(0x8205), w::int32(3)),
            (lid(0x8503), w::boolean(true)),
            (lid(0x8501), w::int32(15)),
            (0x0036, w::int32(2)),
            (0x0017, w::int32(2)),
        ],
    );
    let answer = run(&mut s, &dir, &file(vec![folder("Calendar", "IPF.Appointment", vec![a])]), json!({}));
    adds_up(&answer);
    assert_eq!(answer["events"], 1);
    let e = event(&s, "Dentist");
    assert_eq!(e.start, DateTime::new(Date::new(2026, 3, 2).unwrap(), 9, 0, 0).unwrap());
    assert_eq!(e.end, DateTime::new(Date::new(2026, 3, 2).unwrap(), 9, 30, 0).unwrap());
    assert_eq!(
        (e.location.as_str(), e.show_as, e.reminder_minutes, e.private, e.importance),
        ("High Street", ShowAs::OutOfOffice, Some(15), true, Importance::High)
    );
    assert_eq!(e.description, "Notes of Dentist");
    let calendar = s.mailbox.calendar(e.calendar).unwrap();
    assert_eq!(calendar.name, "old-mail");
    assert!(calendar.origin.is_local());
}

#[test]
fn a_whole_day_is_kept_as_the_days_it_was_in_its_own_zone() {
    let dir = Temp::new("cal-day");
    let mut s = kept_session(&dir);
    // Midnight in New York, 05:00 UTC: the 2nd of March there and here.
    let a = appointment(
        "Holiday",
        at(2026, 3, 2, 5, 0),
        at(2026, 3, 4, 5, 0),
        vec![(lid(0x8215), w::boolean(true)), (lid(0x825E), w::binary(&time_zone_bytes("Eastern Standard Time")))],
    );
    run(&mut s, &dir, &file(vec![folder("Calendar", "IPF.Appointment", vec![a])]), json!({}));
    let e = event(&s, "Holiday");
    assert!(e.all_day);
    assert_eq!((e.start.date, e.end.date), (Date::new(2026, 3, 2).unwrap(), Date::new(2026, 3, 4).unwrap()));
    assert_eq!((e.start.hour, e.end.hour), (0, 0));
}

#[test]
fn a_series_repeats_with_its_deleted_and_changed_days() {
    let dir = Temp::new("cal-series");
    let mut s = kept_session(&dir);
    let first = at(2026, 3, 2, 0, 0);
    let day = |n: i64| minutes(first + n * 86_400);
    // Every Monday 09:00–10:00 until 30 March; the 9th deleted, the 16th moved to 14:00.
    let moved = (day(14) + 840, day(14) + 900, day(14) + 540, Some("Moved".to_string()));
    let blob = recurrence_bytes(0x200B, 1, 1, &[0b10], (0x2021, 5, day(28)), &[day(7), day(14)], day(0), (540, 600), &[moved]);
    let a = appointment(
        "Standup",
        first + 9 * 3600,
        first + 10 * 3600,
        vec![(lid(0x8223), w::boolean(true)), (lid(0x8216), w::binary(&blob)), (lid(0x8260), w::binary(&time_zone_bytes("UTC")))],
    );
    let answer = run(&mut s, &dir, &file(vec![folder("Calendar", "IPF.Appointment", vec![a])]), json!({}));
    adds_up(&answer);
    assert_eq!(answer["events"], 1, "one item in the file");
    let e = event(&s, "Standup");
    let rule = e.recurrence.as_ref().unwrap();
    assert_eq!((rule.freq, rule.interval, rule.until), (Freq::Weekly, 1, Some(Date::new(2026, 3, 30).unwrap())));
    let (from, to) = (Date::new(2026, 3, 1).unwrap(), Date::new(2026, 4, 30).unwrap());
    let days: Vec<u8> = e.occurrences(from, to).iter().map(|o| o.start.date.day).collect();
    assert_eq!(days, vec![2, 23, 30]);
    let m = event(&s, "Moved");
    assert!(m.recurrence.is_none());
    assert_eq!(m.start, DateTime::new(Date::new(2026, 3, 16).unwrap(), 14, 0, 0).unwrap());
    assert_eq!(m.description, "Notes of Standup");
    assert_eq!(s.mailbox.events.len(), 2);
}

#[test]
fn a_monthly_series_starts_on_its_own_day_not_the_day_the_pattern_opens() {
    // As Outlook wrote one: the pattern opens on the 19th, and repeats on the 20th.
    let dir = Temp::new("cal-monthly");
    let mut s = kept_session(&dir);
    let opens = at(2026, 8, 19, 0, 0);
    let blob = recurrence_bytes(0x200C, 2, 1, &[20], (0x2022, 3, minutes(at(2026, 10, 20, 0, 0))), &[], minutes(opens), (525, 645), &[]);
    let a = appointment("Monthly", opens + 525 * 60, opens + 645 * 60, vec![(lid(0x8223), w::boolean(true)), (lid(0x8216), w::binary(&blob))]);
    run(&mut s, &dir, &file(vec![folder("Calendar", "IPF.Appointment", vec![a])]), json!({}));
    let e = event(&s, "Monthly");
    let days: Vec<(u8, u8)> = e
        .occurrences(Date::new(2026, 8, 1).unwrap(), Date::new(2026, 12, 31).unwrap())
        .iter()
        .map(|o| (o.start.date.month, o.start.date.day))
        .collect();
    assert_eq!(days, vec![(8, 20), (9, 20), (10, 20)]);
    assert_eq!((e.start.hour, e.start.minute), (8, 45));
}

#[test]
fn a_series_this_calendar_cannot_keep_is_its_first_occurrence_and_named() {
    let dir = Temp::new("cal-hijri");
    let mut s = kept_session(&dir);
    let first = at(2026, 3, 2, 0, 0);
    let blob = recurrence_bytes(0x200C, 0x0A, 1, &[2], (0x2023, 0, 0x5AE9_80DF), &[], minutes(first), (540, 600), &[]);
    let junk = appointment("Junk", first, first + 60, vec![(lid(0x8223), w::boolean(true)), (lid(0x8216), w::binary(&[1, 2, 3]))]);
    let hijri = appointment("Hijri", first, first + 60, vec![(lid(0x8223), w::boolean(true)), (lid(0x8216), w::binary(&blob))]);
    let answer = run(&mut s, &dir, &file(vec![folder("Calendar", "IPF.Appointment", vec![junk, hijri])]), json!({}));
    adds_up(&answer);
    assert!(event(&s, "Junk").recurrence.is_none() && event(&s, "Hijri").recurrence.is_none());
    let why = notes(&answer);
    assert_eq!(why.iter().filter(|w| w.contains("only its first occurrence")).count(), 2, "{why:?}");
}

#[test]
fn a_meeting_keeps_its_people_as_data() {
    let dir = Temp::new("cal-meeting");
    let mut s = kept_session(&dir);
    let mut a = appointment("Review", at(2026, 3, 2, 9, 0), at(2026, 3, 2, 10, 0), vec![]);
    a.recipients = vec![
        ("Org Anizer".into(), "org@example.org".into(), 1),
        ("Ann".into(), "ann@example.org".into(), 1),
        ("Bo".into(), "bo@example.org".into(), 2),
    ];
    a.recipient_states = vec![(3, 0), (1, 3), (1, 4)];
    run(&mut s, &dir, &file(vec![folder("Calendar", "IPF.Appointment", vec![a])]), json!({}));
    let e = event(&s, "Review");
    assert_eq!(e.organizer.as_ref().unwrap().email, "org@example.org");
    assert_eq!(e.attendees.len(), 2);
    assert_eq!((e.attendees[0].response, e.attendees[0].optional), (Response::Accepted, false));
    assert_eq!((e.attendees[1].response, e.attendees[1].optional), (Response::Declined, true));
    assert!(s.mailbox.messages.values().all(|m| m.subject != "Review"), "nothing is put in an Outbox");
}

#[test]
fn a_task_keeps_its_dates_and_completion_and_is_never_on_a_server() {
    let dir = Temp::new("cal-task");
    let mut s = kept_session(&dir);
    let done = task(
        "Done",
        vec![
            (lid(0x811C), w::boolean(true)),
            (lid(0x810F), w::time(filetime(at(2026, 3, 3, 0, 0)))),
            (lid(0x8105), w::time(filetime(at(2026, 3, 5, 0, 0)))),
        ],
    );
    let open = task("Open", vec![(lid(0x8104), w::time(filetime(at(2026, 4, 1, 0, 0))))]);
    let answer = run(&mut s, &dir, &file(vec![folder("Tasks", "IPF.Task", vec![done, open])]), json!({}));
    adds_up(&answer);
    assert_eq!(answer["tasks"], 2);
    let by = |t: &str| s.mailbox.tasks.values().find(|x| x.title == t).unwrap().clone();
    assert_eq!((by("Done").completed, by("Done").due), (Some(at(2026, 3, 3, 0, 0)), Some(Date::new(2026, 3, 5).unwrap())));
    assert_eq!((by("Open").completed, by("Open").start), (None, Some(Date::new(2026, 4, 1).unwrap())));
    assert_eq!(by("Open").notes, "Task notes");
    assert!(by("Open").origin.is_local());
    assert!(!by("Open").only_here(), "the question about to-dos that exist only here leaves it out");
    assert!(by("Open").kept_here && by("Open").local_only);
}

#[test]
fn a_second_import_adds_nothing() {
    let dir = Temp::new("cal-again");
    let mut s = kept_session(&dir);
    let bytes = file(vec![
        folder("Calendar", "IPF.Appointment", vec![appointment("Once", at(2026, 3, 2, 9, 0), at(2026, 3, 2, 10, 0), vec![])]),
        folder("Tasks", "IPF.Task", vec![task("Todo", vec![])]),
    ]);
    let calendars = s.mailbox.calendars.len();
    run(&mut s, &dir, &bytes, json!({}));
    assert_eq!(s.mailbox.calendars.len(), calendars + 1);
    let again = run(&mut s, &dir, &bytes, json!({}));
    adds_up(&again);
    assert_eq!((again["events"].as_u64(), again["tasks"].as_u64()), (Some(0), Some(0)));
    assert_eq!((s.mailbox.events.len(), s.mailbox.tasks.len(), s.mailbox.calendars.len()), (1, 1, calendars + 1));
}

#[test]
fn undo_takes_the_events_tasks_and_calendar_out() {
    let dir = Temp::new("cal-undo");
    let mut s = kept_session(&dir);
    let calendars = s.mailbox.calendars.len();
    let bytes = file(vec![
        folder("Calendar", "IPF.Appointment", vec![appointment("Once", at(2026, 3, 2, 9, 0), at(2026, 3, 2, 10, 0), vec![])]),
        folder("Tasks", "IPF.Task", vec![task("Todo", vec![])]),
    ]);
    run(&mut s, &dir, &bytes, json!({}));
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!((s.mailbox.events.len(), s.mailbox.tasks.len(), s.mailbox.calendars.len()), (0, 0, calendars));
}

#[test]
fn deleted_items_are_left_out_named() {
    let dir = Temp::new("cal-deleted");
    let mut s = kept_session(&dir);
    let gone = appointment("Gone", at(2026, 3, 2, 9, 0), at(2026, 3, 2, 10, 0), vec![]);
    let answer = run(&mut s, &dir, &file(vec![folder("Deleted Items", "IPF.Note", vec![gone, task("Gone task", vec![])])]), json!({}));
    adds_up(&answer);
    assert!(s.mailbox.events.is_empty() && s.mailbox.tasks.is_empty());
    assert!(answer["leftOutItems"].as_array().unwrap().iter().any(|n| n["why"].as_str().unwrap().contains("Deleted Items")), "{answer}");
}

#[test]
fn a_chosen_calendar_is_used_and_one_on_a_server_refused() {
    let dir = Temp::new("cal-place");
    let mut s = kept_session(&dir);
    let id = s.mailbox.alloc_id();
    s.mailbox.calendars.push(mailcraft_model::Calendar { id, name: "Mine".into(), visible: true, ..Default::default() });
    let bytes = file(vec![folder("Calendar", "IPF.Appointment", vec![appointment("Once", at(2026, 3, 2, 9, 0), at(2026, 3, 2, 10, 0), vec![])])]);
    let calendars = s.mailbox.calendars.len();
    run(&mut s, &dir, &bytes, json!({"places": {"calendar": id}}));
    assert_eq!(event(&s, "Once").calendar, id);
    assert_eq!(s.mailbox.calendars.len(), calendars);

    let mut bare = Session::at(Mailbox::with_local_account("", ""), crate::import_pst_memory_tests::now(), 0);
    let remote = bare.mailbox.alloc_id();
    bare.mailbox.calendars.push(mailcraft_model::Calendar {
        id: remote,
        name: "Server".into(),
        origin: mailcraft_model::Origin { account: 1, id: "x".into(), ..Default::default() },
        ..Default::default()
    });
    let pst = dir.pst(&bytes);
    let err = run_staged(&mut bare, &pst, None, false, &json!({"places": {"calendar": remote}})).unwrap_err().to_string();
    assert!(err.contains("on this computer only"), "{err}");
}

#[test]
fn changed_occurrences_stay_within_what_one_import_may_make() {
    let dir = Temp::new("cal-budget");
    let mut s = kept_session(&dir);
    let first = at(2026, 3, 2, 0, 0);
    let day = |n: i64| minutes(first + n * 86_400);
    let changed: Vec<_> = (0..200).map(|n| (day(n) + 600, day(n) + 660, day(n) + 540, None)).collect();
    let blob = recurrence_bytes(0x200A, 0, 1440, &[], (0x2023, 0, 0x5AE9_80DF), &[], day(0), (540, 600), &changed);
    let mut a = appointment("Big", first + 9 * 3600, first + 10 * 3600, vec![(lid(0x8223), w::boolean(true)), (lid(0x8216), w::binary(&blob))]);
    a.body = "x".repeat(1 << 20);
    let answer = run(&mut s, &dir, &file(vec![folder("Calendar", "IPF.Appointment", vec![a])]), json!({}));
    adds_up(&answer);
    let held: usize = s.mailbox.events.values().map(|e| e.description.len()).sum();
    assert!(held as u64 <= crate::import_pst::item::RTF_TEXT_BUDGET + (2 << 20), "{held}");
    assert!(notes(&answer).iter().any(|n| n.contains("changed occurrences are left out")), "{answer}");
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(64))]
    /// A damaged series never makes the import panic, and the item is accounted for.
    #[test]
    fn a_damaged_series_never_panics(flips in proptest::collection::vec((proptest::prelude::any::<proptest::sample::Index>(), proptest::prelude::any::<u8>()), 1..16)) {
        let first = at(2026, 3, 2, 0, 0);
        let day = |n: i64| minutes(first + n * 86_400);
        let moved = (day(14) + 840, day(14) + 900, day(14) + 540, Some("Moved".to_string()));
        let mut blob = recurrence_bytes(0x200B, 1, 1, &[0b10], (0x2021, 5, day(28)), &[day(7), day(14)], day(0), (540, 600), &[moved]);
        for (i, v) in flips {
            let i = i.index(blob.len());
            blob[i] = v;
        }
        let dir = Temp::new("cal-hostile");
        let mut s = kept_session(&dir);
        let a = appointment("Hostile", first, first + 3600, vec![(lid(0x8223), w::boolean(true)), (lid(0x8216), w::binary(&blob))]);
        let answer = run(&mut s, &dir, &file(vec![folder("Calendar", "IPF.Appointment", vec![a])]), json!({}));
        adds_up(&answer);
        // Whatever the rule says, asking when it happens answers.
        for e in s.mailbox.events.values() {
            let _ = e.occurrences(Date::new(2026, 1, 1).unwrap(), Date::new(2027, 1, 1).unwrap());
        }
    }
}
