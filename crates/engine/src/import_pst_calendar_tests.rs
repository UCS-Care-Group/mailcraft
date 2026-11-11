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
use crate::import_pst::calendar::{PSETID_APPOINTMENT, PSETID_COMMON, PSETID_LOG, PSETID_MEETING, PSETID_NOTE};
use crate::import_pst::item::PSETID_TASK;
use crate::import_pst::run_staged;
use crate::import_pst_memory_tests::{Temp, kept_session};

/// The named properties the writer is given, in the order of their ids (0x8000 on).
const NAMED: [([u8; 16], u32); 22] = [
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
    (PSETID_NOTE, 0x8B00),
    (PSETID_LOG, 0x8700),
    (PSETID_LOG, 0x8706),
    (PSETID_LOG, 0x8707),
    (PSETID_LOG, 0x8708),
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

/// The same for a time before 1970, whose division must round down.
fn minutes_i(ts: i64) -> u32 {
    (ts.div_euclid(60) + 194_074_560) as u32
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
        let sum: u64 = ["added", "already", "leftOut"].iter().map(|k| r[*k].as_u64().unwrap()).sum();
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
    assert!(held as u64 <= crate::import_pst::item::PLAN_TEXT_BUDGET + (2 << 20), "{held}");
    assert!(notes(&answer).iter().any(|n| n.contains("changed occurrences are left out")), "{answer}");
}

const KEYWORDS: u16 = 0x8000 + NAMED.len() as u16;

fn keywords(items: &[String]) -> Value {
    let mut b = (items.len() as u32).to_le_bytes().to_vec();
    let mut at = 4 + 4 * items.len() as u32;
    let texts: Vec<Vec<u8>> = items.iter().map(|t| t.encode_utf16().flat_map(u16::to_le_bytes).collect()).collect();
    for t in &texts {
        b.extend_from_slice(&at.to_le_bytes());
        at += t.len() as u32;
    }
    texts.iter().for_each(|t| b.extend_from_slice(t));
    Value { ty: 0x101F, bytes: b }
}

fn repeating(blob: &[u8]) -> Vec<(u16, Value)> {
    vec![(lid(0x8223), w::boolean(true)), (lid(0x8216), w::binary(blob))]
}

fn on(y: i32, m: u8, d: u8) -> Date {
    Date::new(y, m, d).unwrap()
}

fn days(s: &Session, title: &str, from: Date, to: Date) -> Vec<Date> {
    event(s, title).occurrences(from, to).iter().map(|o| o.start.date).collect()
}

fn import_one(name: &str, a: MessageSpec) -> (Temp, Session, Json) {
    let dir = Temp::new(name);
    let mut s = kept_session(&dir);
    let answer = run(&mut s, &dir, &file(vec![folder("Calendar", "IPF.Appointment", vec![a])]), json!({}));
    adds_up(&answer);
    (dir, s, answer)
}

const NEVER: (u32, u32, u32) = (0x2023, 0, 0x5AE9_80DF);

#[test]
fn the_keywords_of_changed_occurrences_count_against_the_budget() {
    let first = at(2026, 3, 2, 0, 0);
    let day = |n: i64| minutes(first + n * 86_400);
    let changed: Vec<_> = (0..4000).map(|n| (day(n) + 600, day(n) + 660, day(n) + 540, None)).collect();
    let blob = recurrence_bytes(0x200A, 0, 1440, &[], NEVER, &[], day(0), (540, 600), &changed);
    let words: Vec<String> = (0..100).map(|n| format!("{n:03}{}", "k".repeat(110))).collect();
    let mut extra = repeating(&blob);
    extra.push((KEYWORDS, keywords(&words)));
    let (_dir, s, answer) = import_one("cal-keywords", appointment("Tagged", first + 9 * 3600, first + 10 * 3600, extra));
    let held: usize = s.mailbox.events.values().flat_map(|e| e.categories.iter()).map(String::len).sum();
    assert!(held as u64 <= crate::import_pst::item::PLAN_TEXT_BUDGET, "{held}");
    assert!(notes(&answer).iter().any(|n| n.contains("changed occurrences are left out")), "{answer}");
}

#[test]
fn events_and_tasks_together_stay_within_what_one_import_may_bring() {
    let dir = Temp::new("cal-total");
    let mut s = kept_session(&dir);
    let start = at(2026, 3, 2, 9, 0);
    let big = |mut m: MessageSpec| {
        m.body = "x".repeat(1 << 20);
        m
    };
    let events = (0..7).map(|n| big(appointment(&format!("E{n}"), start, start + 60, vec![]))).collect();
    let tasks = (0..7).map(|n| big(task(&format!("T{n}"), vec![]))).collect();
    let answer = run(&mut s, &dir, &file(vec![folder("Calendar", "IPF.Appointment", events), folder("Tasks", "IPF.Task", tasks)]), json!({}));
    adds_up(&answer);
    let held = s.mailbox.events.values().map(|e| e.description.len()).sum::<usize>() + s.mailbox.tasks.values().map(|t| t.notes.len()).sum::<usize>();
    assert!(held as u64 <= crate::import_pst::item::PLAN_TEXT_BUDGET, "{held}");
    let why: Vec<&str> = answer["leftOutItems"].as_array().unwrap().iter().map(|n| n["why"].as_str().unwrap()).collect();
    assert!(!why.is_empty() && why.iter().all(|w| w.contains("as much calendar and task text")), "{why:?}");
}

#[test]
fn a_birthday_from_before_1970_comes_in_and_keeps_repeating() {
    let first = at(1965, 8, 5, 0, 0);
    let blob = recurrence_bytes(0x200D, 2, 12, &[5], NEVER, &[], minutes_i(first), (0, 1440), &[]);
    let mut extra = repeating(&blob);
    extra.push((lid(0x8215), w::boolean(true)));
    let (_dir, s, answer) = import_one("cal-1965", appointment("Birthday", first, first + 86_400, extra));
    assert_eq!(answer["events"], 1, "{answer}");
    assert!(event(&s, "Birthday").recurrence.is_some(), "{answer}");
    assert_eq!(days(&s, "Birthday", on(2026, 1, 1), on(2026, 12, 31)), vec![on(2026, 8, 5)]);
}

#[test]
fn a_task_with_outlooks_no_date_has_no_date() {
    let dir = Temp::new("task-none");
    let mut s = kept_session(&dir);
    let none = w::time(0x0CB3_4557_A3DD_4000);
    let t = task("Open ended", vec![(lid(0x8104), none.clone()), (lid(0x8105), none)]);
    run(&mut s, &dir, &file(vec![folder("Tasks", "IPF.Task", vec![t])]), json!({}));
    let t = s.mailbox.tasks.values().find(|t| t.title == "Open ended").unwrap();
    assert_eq!((t.start, t.due), (None, None));
}

#[test]
fn a_series_starts_where_the_item_says_not_where_the_pattern_opens() {
    let opens = minutes(at(2026, 8, 19, 0, 0));
    let item = |title: &str, y, m, d, blob: &[u8]| appointment(title, at(y, m, d, 9, 0), at(y, m, d, 10, 0), repeating(blob));
    let yearly = recurrence_bytes(0x200D, 2, 12, &[5], NEVER, &[], opens, (540, 600), &[]);
    let (_a, s, _) = import_one("cal-yearly", item("Y", 2027, 8, 5, &yearly));
    assert_eq!(days(&s, "Y", on(2026, 1, 1), on(2028, 12, 31)), vec![on(2027, 8, 5), on(2028, 8, 5)]);
    let two = recurrence_bytes(0x200C, 2, 2, &[5], NEVER, &[], opens, (540, 600), &[]);
    let (_b, s, _) = import_one("cal-two-months", item("M", 2026, 10, 5, &two));
    assert_eq!(days(&s, "M", on(2026, 8, 1), on(2026, 12, 31)), vec![on(2026, 10, 5), on(2026, 12, 5)]);
    let counted = recurrence_bytes(0x200C, 3, 1, &[0x7F, 2], NEVER, &[], opens, (540, 600), &[]);
    let (_c, s, _) = import_one("cal-counted", item("N", 2026, 9, 2, &counted));
    assert_eq!(days(&s, "N", on(2026, 8, 1), on(2026, 10, 31)), vec![on(2026, 9, 2), on(2026, 10, 2)]);
}

#[test]
fn the_31st_starting_in_a_short_month_keeps_that_first_day() {
    let opens = minutes(at(2026, 4, 10, 0, 0));
    let blob = recurrence_bytes(0x200C, 2, 1, &[31], (0x2021, 3, minutes(at(2026, 5, 31, 0, 0))), &[], opens, (540, 600), &[]);
    let a = appointment("Month end", at(2026, 4, 30, 9, 0), at(2026, 4, 30, 10, 0), repeating(&blob));
    let (_d, s, _) = import_one("cal-31", a);
    let mut all: Vec<Date> = s.mailbox.events.values().flat_map(|e| e.occurrences(on(2026, 4, 1), on(2026, 7, 31))).map(|o| o.start.date).collect();
    all.sort();
    assert_eq!(all, vec![on(2026, 4, 30), on(2026, 5, 31)]);
}

#[test]
fn a_fortnight_counted_from_another_first_day_is_not_guessed() {
    let first = at(2026, 3, 1, 0, 0);
    // Every two weeks on Sunday and Monday, weeks starting on Sunday.
    let mut blob = recurrence_bytes(0x200B, 1, 2, &[0b11], NEVER, &[], minutes(first), (540, 600), &[]);
    blob[34] = 0;
    let (_d, s, answer) = import_one("cal-dow", appointment("Pair", first + 9 * 3600, first + 10 * 3600, repeating(&blob)));
    assert!(event(&s, "Pair").recurrence.is_none());
    assert!(notes(&answer).iter().any(|n| n.contains("only its first occurrence")), "{answer}");
    // Sunday alone is the same fortnight however the week is counted.
    let mut one = recurrence_bytes(0x200B, 1, 2, &[0b1], NEVER, &[], minutes(first), (540, 600), &[]);
    one[34] = 0;
    let (_e, s, _) = import_one("cal-dow1", appointment("Sunday", first + 9 * 3600, first + 10 * 3600, repeating(&one)));
    assert_eq!(days(&s, "Sunday", on(2026, 3, 1), on(2026, 3, 31)), vec![on(2026, 3, 1), on(2026, 3, 15), on(2026, 3, 29)]);
}

#[test]
fn a_series_without_a_zone_keeps_the_instant_in_the_file() {
    // 09:00 by its own clock, which the file's 14:00 UTC shows was five hours behind.
    let opens = at(2026, 3, 2, 0, 0);
    let blob = recurrence_bytes(0x200A, 0, 1440, &[], NEVER, &[], minutes(opens), (540, 600), &[]);
    let (_d, s, answer) = import_one("cal-nozone", appointment("Far away", opens + 14 * 3600, opens + 15 * 3600, repeating(&blob)));
    assert_eq!(event(&s, "Far away").start.hour, 14, "{answer}");
    assert!(notes(&answer).iter().any(|n| n.contains("time zone")), "{answer}");
}

#[test]
fn a_series_in_another_zone_is_shown_by_this_computers_clock() {
    let dir = Temp::new("cal-zones");
    let mut s = kept_session(&dir);
    s.prefs.time_zone = "Europe/London".to_string();
    s.clock.offset = None;
    // Late on a New York evening: the next day in London.
    let opens = at(2026, 1, 12, 0, 0);
    let blob = recurrence_bytes(0x200B, 1, 1, &[0b10], (0x2021, 2, minutes(at(2026, 1, 19, 0, 0))), &[], minutes(opens), (1320, 1380), &[]);
    let mut extra = repeating(&blob);
    extra.push((lid(0x8260), w::binary(&time_zone_bytes("Eastern Standard Time"))));
    let a = appointment("Evening", opens + 27 * 3600, opens + 28 * 3600, extra);
    run(&mut s, &dir, &file(vec![folder("Calendar", "IPF.Appointment", vec![a])]), json!({}));
    let e = event(&s, "Evening");
    assert_eq!((e.start.date, e.start.hour), (on(2026, 1, 13), 3));
}

#[test]
fn a_second_import_after_the_series_is_deleted_does_not_repeat_what_was_moved() {
    let dir = Temp::new("cal-outlived");
    let mut s = kept_session(&dir);
    let first = at(2026, 3, 2, 0, 0);
    let day = |n: i64| minutes(first + n * 86_400);
    let moved = (day(7) + 840, day(7) + 900, day(7) + 540, Some("Moved".to_string()));
    let blob = recurrence_bytes(0x200B, 1, 1, &[0b10], NEVER, &[], day(0), (540, 600), &[moved]);
    let bytes = file(vec![folder("Calendar", "IPF.Appointment", vec![appointment("Weekly", first + 9 * 3600, first + 10 * 3600, repeating(&blob))])]);
    run(&mut s, &dir, &bytes, json!({}));
    let series = event(&s, "Weekly").id;
    s.mailbox.events.remove(&series);
    run(&mut s, &dir, &bytes, json!({}));
    assert_eq!(s.mailbox.events.values().filter(|e| e.title == "Moved").count(), 1);
    assert_eq!(s.mailbox.events.values().filter(|e| e.title == "Weekly").count(), 1);
}

#[test]
fn a_series_that_ends_before_it_starts_is_its_first_occurrence_and_named() {
    let first = at(2026, 3, 2, 0, 0);
    let blob = recurrence_bytes(0x200A, 0, 1440, &[], (0x2021, 1, minutes(at(2026, 2, 1, 0, 0))), &[], minutes(first), (540, 600), &[]);
    let (_d, s, answer) = import_one("cal-ended", appointment("Ended", first + 9 * 3600, first + 10 * 3600, repeating(&blob)));
    assert_eq!(days(&s, "Ended", on(2026, 3, 1), on(2026, 3, 31)), vec![on(2026, 3, 2)]);
    assert!(notes(&answer).iter().any(|n| n.contains("only its first occurrence")), "{answer}");
}

#[test]
fn a_calendar_removed_meanwhile_stops_the_import() {
    use crate::import_pst::{Batch, Staged};
    let dir = Temp::new("cal-gone");
    let mut s = kept_session(&dir);
    let id = s.mailbox.alloc_id();
    s.mailbox.calendars.push(mailcraft_model::Calendar { id, name: "Chosen".into(), visible: true, ..Default::default() });
    let batch = || {
        let mut b = Batch::default();
        b.events = vec![mailcraft_model::Event { title: "One".into(), calendar: id, ..Default::default() }];
        b.identity = "x".into();
        b
    };
    let params = json!({"staged": true, "name": "old-mail", "places": {"calendar": id}});
    s.staged_pst = Some(Staged::Batch(batch()));
    s.run("file.importPst", &params).unwrap();
    let before = s.mailbox.events.len();
    s.mailbox.calendars.retain(|c| c.id != id);
    s.staged_pst = Some(Staged::Batch(batch()));
    let e = s.run("file.importPst", &params).unwrap_err().to_string();
    assert!(e.contains("calendar"), "{e}");
    assert_eq!(s.mailbox.events.len(), before);
}

#[test]
fn what_the_reader_left_out_of_a_meeting_is_named() {
    let start = at(2026, 3, 2, 9, 0);
    let mut a = appointment("Crowded", start, start + 3600, vec![]);
    a.recipients = (0..1010).map(|n| (format!("P{n}"), format!("p{n}@example.org"), 1)).collect();
    let (_d, _s, answer) = import_one("cal-crowd", a);
    assert!(notes(&answer).iter().any(|n| n.contains("past the limit")), "{answer}");
}

#[test]
fn a_gregorian_variant_keeps_repeating() {
    let first = at(2026, 3, 2, 0, 0);
    let mut blob = recurrence_bytes(0x200A, 0, 1440, &[], NEVER, &[], minutes(first), (540, 600), &[]);
    blob[8] = 2;
    let (_d, s, _) = import_one("cal-us", appointment("Daily", first + 9 * 3600, first + 10 * 3600, repeating(&blob)));
    assert!(event(&s, "Daily").recurrence.is_some());
}

fn sticky(text: &str, color: i32) -> MessageSpec {
    MessageSpec {
        class: "IPM.StickyNote".into(),
        subject: "First line".into(),
        body: text.into(),
        extra: vec![(lid(0x8B00), w::int32(color))],
        ..Default::default()
    }
}

fn logged(subject: &str) -> MessageSpec {
    let start = at(2026, 3, 2, 9, 0);
    MessageSpec {
        class: "IPM.Activity".into(),
        subject: subject.into(),
        body: "What was said".into(),
        extra: vec![
            (lid(0x8700), w::unicode("Phone call")),
            (lid(0x8706), w::time(filetime(start))),
            (lid(0x8708), w::time(filetime(start + 1800))),
            (lid(0x8707), w::int32(30)),
        ],
        recipients: vec![("Ada Fernsby".into(), "ada@example.org".into(), 1)],
        ..Default::default()
    }
}

fn notes_file() -> Vec<u8> {
    file(vec![
        folder("Notes", "IPF.StickyNote", vec![sticky("Buy stamps\nand envelopes", 0), sticky("Ring the plumber", 2)]),
        folder("Journal", "IPF.Journal", vec![logged("Call with Ada")]),
    ])
}

#[test]
fn notes_and_journal_entries_are_kept_and_can_be_found() {
    let dir = Temp::new("notes-in");
    let mut s = kept_session(&dir);
    let answer = run(&mut s, &dir, &notes_file(), json!({}));
    adds_up(&answer);
    assert_eq!(
        (answer["stickyNotes"].as_u64(), answer["journalEntries"].as_u64(), answer["messages"].as_u64()),
        (Some(2), Some(1), Some(0)),
        "{answer}"
    );
    let found = s.run("notes.list", &json!({"query": "STAMPS"})).unwrap();
    assert_eq!(found["total"], 1, "{found}");
    assert_eq!((found["notes"][0]["subject"].as_str(), found["notes"][0]["color"].as_str()), (Some("Buy stamps"), Some("blue")));
    let whole = s.run("notes.get", &json!({"id": found["notes"][0]["id"]})).unwrap();
    assert_eq!(whole["body"], "Buy stamps\nand envelopes");
    assert_eq!(s.run("notes.list", &json!({"query": "stamps plumber"})).unwrap()["total"], 0, "every word must be in the one note");
    let entries = s.run("journal.list", &json!({"query": "phone"})).unwrap();
    assert_eq!(entries["total"], 1, "{entries}");
    let e = s.run("journal.get", &json!({"id": entries["entries"][0]["id"]})).unwrap();
    assert_eq!(
        (e["subject"].as_str(), e["value"].as_str(), e["minutes"].as_u64(), e["body"].as_str()),
        (Some("Call with Ada"), Some("Phone call"), Some(30), Some("What was said"))
    );
    assert_eq!(e["start"], "2026-03-02T09:00:00Z");
    assert_eq!(e["from"][0], "Ada Fernsby <ada@example.org>");
    assert!(s.run("notes.get", &json!({"id": 999_999})).is_err());
}

#[test]
fn a_second_import_keeps_notes_once_and_a_dry_run_keeps_none() {
    let dir = Temp::new("notes-again");
    let mut s = kept_session(&dir);
    let bytes = notes_file();
    let pst = dir.pst(&bytes);
    let look = run_staged(&mut s, &pst, None, true, &json!({})).unwrap();
    assert_eq!((look["stickyNotes"].as_u64(), s.mailbox.notes.len()), (Some(2), 0));
    run(&mut s, &dir, &bytes, json!({}));
    let again = run(&mut s, &dir, &bytes, json!({}));
    adds_up(&again);
    assert_eq!((again["added"].as_u64(), again["already"].as_u64()), (Some(0), Some(3)), "{again}");
    assert_eq!((s.mailbox.notes.len(), s.mailbox.journal.len()), (2, 1));
}

#[test]
fn undo_takes_the_notes_and_journal_out() {
    let dir = Temp::new("notes-undo");
    let mut s = kept_session(&dir);
    run(&mut s, &dir, &notes_file(), json!({}));
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!((s.mailbox.notes.len(), s.mailbox.journal.len()), (0, 0));
}

#[test]
fn notes_and_journal_outlast_a_save() {
    let dir = Temp::new("notes-save");
    let mut s = kept_session(&dir);
    run(&mut s, &dir, &notes_file(), json!({}));
    crate::store::save(&dir.mailbox(), &s.mailbox).unwrap();
    let mut back = crate::store::open(&dir.mailbox()).unwrap().mailbox;
    assert_eq!((back.notes.len(), back.journal.len()), (2, 1));
    assert_eq!(back.notes, s.mailbox.notes);
    let top = *back.notes.keys().chain(back.journal.keys()).max().unwrap();
    assert!(back.alloc_id() > top, "an id is never used twice");
}

#[test]
fn notes_in_deleted_items_are_left_out_named() {
    let dir = Temp::new("notes-deleted");
    let mut s = kept_session(&dir);
    let answer = run(&mut s, &dir, &file(vec![folder("Deleted Items", "IPF.Note", vec![sticky("Gone", 3), logged("Gone too")])]), json!({}));
    adds_up(&answer);
    assert_eq!((s.mailbox.notes.len(), s.mailbox.journal.len()), (0, 0));
}

#[test]
fn notes_stay_within_what_one_import_may_keep() {
    let dir = Temp::new("notes-cap");
    let mut s = kept_session(&dir);
    let big = "n".repeat(600 << 10);
    let many = (0..6).map(|_| sticky(&big, 3)).collect();
    let answer = run(&mut s, &dir, &file(vec![folder("Notes", "IPF.StickyNote", many)]), json!({}));
    adds_up(&answer);
    let held: usize = s.mailbox.notes.values().map(|n| n.text.len()).sum();
    assert!(held as u64 <= crate::import_pst::item::KEPT_TEXT_BUDGET, "{held}");
    assert!(answer["leftOut"].as_u64().unwrap() > 0, "{answer}");
    let why = answer["leftOutItems"][0]["why"].as_str().unwrap();
    assert!(why.contains("notes and journal entries"), "{why}");
}

#[test]
fn a_mailbox_full_of_notes_takes_no_more_from_any_file() {
    let dir = Temp::new("notes-total");
    let mut s = kept_session(&dir);
    let full = usize::try_from(crate::import_pst::item::KEPT_TOTAL_BUDGET).unwrap().saturating_sub(1000);
    s.mailbox.add_note(mailcraft_model::Note { text: "n".repeat(full), ..Default::default() });
    let answer = run(&mut s, &dir, &file(vec![folder("Notes", "IPF.StickyNote", vec![sticky(&"m".repeat(4000), 3)])]), json!({}));
    adds_up(&answer);
    assert_eq!((answer["stickyNotes"].as_u64(), answer["leftOut"].as_u64()), (Some(0), Some(1)), "{answer}");
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
