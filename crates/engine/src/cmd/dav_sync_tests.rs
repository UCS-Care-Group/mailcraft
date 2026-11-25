// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendars down from a calendar server (#209 D5): what is asked, what is taken in, and what is
//! never believed.

use mailcraft_model::{DavChange, DavDownload, DavListing, DavProblem, DavRead, DavReadState, Id, Rights};
use mailcraft_time::Date;
use serde_json::{Value, json};

use crate::Session;

const COLLECTION: &str = "https://dav.example.org/cal/ada/work/";

fn session() -> (Session, Id) {
    let mut s = Session::empty();
    s.dav_enabled = true;
    s.clock.offset = Some(0);
    let id = s
        .run("account.add", &json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}, "outgoing": {"host": "smtp.example.org"}}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    s.run("account.davSet", &json!({"id": id, "calendarHomes": ["https://dav.example.org/cal/ada/"]})).unwrap();
    (s, id)
}

fn event(uid: &str, day: u32, title: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTART:202611{day:02}T100000Z\r\nDTEND:202611{day:02}T110000Z\r\nSUMMARY:{title}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn change(name: &str, etag: &str, raw: &str) -> DavChange {
    DavChange { href: format!("{COLLECTION}{name}"), etag: etag.into(), raw: Some(raw.into()), gone: false }
}

fn gone(name: &str) -> DavChange {
    DavChange { href: format!("{COLLECTION}{name}"), gone: true, ..Default::default() }
}

fn listing() -> DavListing {
    DavListing {
        calendars: vec![mailcraft_model::DavCalendar { href: COLLECTION.into(), name: "Work".into(), read_only: false, ..Default::default() }],
        complete: true,
        capped: false,
    }
}

/// A read: with `listed`, of a whole listing of the collection (what isn't among `changes` is
/// gone), else of changes alone.
fn read(changes: Vec<DavChange>, listed: bool, complete: bool, token: &str) -> DavRead {
    let members = listed.then(|| changes.iter().filter(|c| !c.gone).map(|c| c.href.clone()).collect());
    DavRead {
        href: COLLECTION.into(),
        state: DavReadState::Read,
        complete,
        sync_token: token.into(),
        ctag: if complete { "ctag-1".into() } else { String::new() },
        changes,
        members,
    }
}

/// The settings a read of the account is made under now.
fn server_of(s: &Session, id: Id) -> String {
    let a = s.mailbox.account(id).unwrap();
    a.dav.as_ref().unwrap().identity(&a.email)
}

fn apply(s: &mut Session, id: Id, listing: Option<DavListing>, reads: Vec<DavRead>) -> Value {
    let server = server_of(s, id);
    let generation = s.dav_generation.get(&id).copied().unwrap_or(0);
    let download =
        DavDownload { account: id, email: "ada@example.org".into(), server, generation, listing, collections: reads, ..Default::default() };
    s.run("sync.applyDav", &json!({"result": download})).unwrap()
}

fn titles(s: &Session) -> Vec<String> {
    let mut t: Vec<String> = s.mailbox.events.values().filter(|e| e.origin.account != 0).map(|e| e.title.clone()).collect();
    t.sort();
    t
}

#[test]
fn it_asks_what_the_settings_name_and_the_calendars_it_knows() {
    let mut s = Session::empty();
    s.dav_enabled = true;
    assert!(s.run("sync.davRequest", &json!({})).is_err(), "no account has a server");
    let (mut s, id) = session();
    let ask = s.run("sync.davRequest", &json!({})).unwrap();
    assert_eq!(ask["ask"]["account"], id);
    assert_eq!(ask["ask"]["username"], "ada@example.org", "the account's own name when none is set");
    assert_eq!(ask["ask"]["listHomes"], true, "nothing is known yet");
    assert_eq!(ask["ask"]["homes"], json!(["https://dav.example.org/cal/ada/"]));
    apply(&mut s, id, Some(listing()), vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], true, true, "")]);
    let ask = s.run("sync.davRequest", &json!({})).unwrap();
    assert_eq!(ask["ask"]["listHomes"], false, "listed a moment ago");
    // Without a token the read is told what is here, so that only what changed is fetched.
    assert_eq!(ask["ask"]["collections"][0]["known"], json!([{"href": format!("{COLLECTION}a.ics"), "etag": "\"1\""}]));
    assert_eq!(ask["ask"]["collections"][0]["ctag"], "ctag-1");
    apply(&mut s, id, None, vec![read(vec![], false, true, "tok-1")]);
    let ask = s.run("sync.davRequest", &json!({"relist": true})).unwrap();
    assert_eq!(ask["ask"]["listHomes"], true);
    assert_eq!(ask["ask"]["collections"][0]["syncToken"], "tok-1");
    assert_eq!(ask["ask"]["server"], server_of(&s, id), "the settings the read is made under");
}

#[test]
fn a_first_sync_makes_a_read_only_calendar_and_its_events_under_their_addresses() {
    let (mut s, id) = session();
    let said = apply(
        &mut s,
        id,
        Some(listing()),
        vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A")), change("b.ics", "\"2\"", &event("b", 3, "B"))], true, true, "tok-1")],
    );
    assert_eq!((said["added"].clone(), said["calendars"].clone(), said["problem"].clone()), (json!(2), json!(1), json!("none")));
    let calendar = s.mailbox.calendars.iter().find(|c| c.origin.account == id).unwrap();
    assert_eq!((calendar.name.as_str(), calendar.rights, calendar.origin.id.as_str()), ("Work", Rights::Read, COLLECTION));
    assert_eq!(titles(&s), vec!["A", "B"]);
    let a = s.mailbox.events.values().find(|e| e.title == "A").unwrap();
    assert_eq!((a.origin.id.as_str(), a.origin.change_key.as_str(), a.calendar), (format!("{COLLECTION}a.ics").as_str(), "\"1\"", calendar.id));
    let state = &s.mailbox.account(id).unwrap().dav.as_ref().unwrap().collections[0];
    assert_eq!((state.sync_token.as_str(), state.ctag.as_str(), state.read_only), ("tok-1", "ctag-1", false));
    // Taking the same in again changes nothing.
    let again = apply(&mut s, id, None, vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], false, true, "tok-1")]);
    assert_eq!((again["added"].clone(), again["updated"].clone()), (json!(0), json!(0)));
    // The calendar is the server's: nothing here can change it yet.
    assert!(!s.mailbox.calendars.iter().any(|c| c.origin.account == id && c.rights.can_edit()));
}

#[test]
fn a_delta_changes_adds_and_removes_and_the_token_moves_with_it() {
    let (mut s, id) = session();
    apply(
        &mut s,
        id,
        Some(listing()),
        vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A")), change("b.ics", "\"2\"", &event("b", 3, "B"))], true, true, "tok-1")],
    );
    let said = apply(
        &mut s,
        id,
        None,
        vec![read(
            vec![change("a.ics", "\"3\"", &event("a", 2, "A renamed")), change("c.ics", "\"4\"", &event("c", 4, "C")), gone("b.ics")],
            false,
            true,
            "tok-2",
        )],
    );
    assert_eq!((said["added"].clone(), said["updated"].clone(), said["removed"].clone()), (json!(1), json!(1), json!(1)));
    assert_eq!(titles(&s), vec!["A renamed", "C"]);
    assert_eq!(s.mailbox.account(id).unwrap().dav.as_ref().unwrap().collections[0].sync_token, "tok-2");
}

#[test]
fn a_deletion_is_believed_only_from_a_whole_listing_or_the_servers_own_word() {
    let (mut s, id) = session();
    apply(
        &mut s,
        id,
        Some(listing()),
        vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A")), change("b.ics", "\"2\"", &event("b", 3, "B"))], true, true, "")],
    );
    // A read without a whole listing leaves what it didn't name alone, however it ended.
    for complete in [false, true] {
        apply(&mut s, id, None, vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], false, complete, "")]);
        assert_eq!(titles(&s), vec!["A", "B"]);
    }
    // A listing too long to be one is none.
    let mut long = read(vec![], true, false, "");
    long.members = Some((0..=mailcraft_model::dav_down::MAX_RESOURCES).map(|n| format!("{COLLECTION}{n}.ics")).collect());
    apply(&mut s, id, None, vec![long]);
    assert_eq!(titles(&s), vec!["A", "B"]);
    // A whole one removes what isn't in it (even when fetching what changed stopped short).
    let said = apply(&mut s, id, None, vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], true, false, "")]);
    assert_eq!(said["removed"], 1);
    assert_eq!(titles(&s), vec!["A"]);
    // And the server's own "gone" does.
    apply(&mut s, id, None, vec![read(vec![gone("a.ics")], false, true, "t")]);
    assert!(titles(&s).is_empty());
}

#[test]
fn a_series_with_changed_and_skipped_occurrences_is_shown_right() {
    let (mut s, id) = session();
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
        BEGIN:VEVENT\r\nUID:s1\r\nDTSTART:20261102T100000Z\r\nDTEND:20261102T110000Z\r\nRRULE:FREQ=DAILY;COUNT=5\r\nEXDATE:20261104T100000Z\r\nSUMMARY:Standup\r\nEND:VEVENT\r\n\
        BEGIN:VEVENT\r\nUID:s1\r\nRECURRENCE-ID:20261103T100000Z\r\nDTSTART:20261103T150000Z\r\nDTEND:20261103T160000Z\r\nSUMMARY:Moved\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let said = apply(&mut s, id, Some(listing()), vec![read(vec![change("s.ics", "\"1\"", ics)], true, true, "")]);
    assert_eq!(said["added"], 2, "the series and its changed occurrence");
    let master = s.mailbox.events.values().find(|e| e.title == "Standup").unwrap();
    assert_eq!(master.origin.id, format!("{COLLECTION}s.ics"));
    let moved = s.mailbox.events.values().find(|e| e.title == "Moved").unwrap();
    assert_eq!(
        (moved.origin.id.as_str(), moved.origin.series.as_str()),
        (format!("{COLLECTION}s.ics#s1#20261103T100000Z").as_str(), format!("{COLLECTION}s.ics").as_str())
    );
    assert_eq!(moved.start.hour, 15);
    let days: Vec<u8> =
        master.occurrences(Date::new(2026, 11, 1).unwrap(), Date::new(2026, 11, 10).unwrap()).iter().map(|o| o.start.date.day).collect();
    assert_eq!(days, vec![2, 5, 6], "the skipped day and the changed one are not the series'");
    // The same resource again, and then the override gone: the key follows the document.
    let again = apply(&mut s, id, None, vec![read(vec![change("s.ics", "\"2\"", ics)], false, true, "t")]);
    assert_eq!((again["added"].clone(), again["updated"].clone()), (json!(0), json!(2)), "a new ETag is a new mark on both");
    let plain = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:s1\r\nDTSTART:20261102T100000Z\r\nDTEND:20261102T110000Z\r\nRRULE:FREQ=DAILY;COUNT=5\r\nSUMMARY:Standup\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let said = apply(&mut s, id, None, vec![read(vec![change("s.ics", "\"3\"", plain)], false, true, "t")]);
    assert_eq!(said["removed"], 1);
    assert_eq!(titles(&s), vec!["Standup"]);
}

#[test]
fn a_meeting_is_shown_with_its_people_and_the_owner_as_a_guest() {
    let (mut s, id) = session();
    let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:m\r\nDTSTART:20261102T100000Z\r\nDTEND:20261102T110000Z\r\nSUMMARY:Review\r\nORGANIZER:mailto:grace@example.org\r\nATTENDEE;PARTSTAT=ACCEPTED:mailto:ada@example.org\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    apply(&mut s, id, Some(listing()), vec![read(vec![change("m.ics", "\"1\"", ics)], true, true, "")]);
    let m = s.mailbox.events.values().find(|e| e.title == "Review").unwrap();
    assert!(m.origin.guest && m.is_meeting() && m.organizer.is_some());
}

#[test]
fn an_address_off_the_collection_or_the_origin_is_never_taken_in() {
    let (mut s, id) = session();
    let raw = event("x", 2, "Stolen");
    let bad = |href: &str| DavChange { href: href.into(), etag: "\"1\"".into(), raw: Some(raw.clone()), gone: false };
    let reads = vec![read(
        vec![
            bad("https://dav.example.org/cal/ada/other/x.ics"),
            bad("https://evil.example.net/cal/ada/work/x.ics"),
            bad("https://dav.example.org/cal/ada/work/../other/x.ics"),
            bad("https://dav.example.org/cal/ada/work/%2e%2e/other/x.ics"),
            bad("https://dav.example.org/cal/ada/work/"),
            bad("http://dav.example.org/cal/ada/work/x.ics"),
            bad("https://outlook.office365.com/cal/ada/work/x.ics"),
            change("ok.ics", "\"1\"", &event("ok", 2, "Fine")),
        ],
        false,
        true,
        "t",
    )];
    let said = apply(&mut s, id, Some(listing()), reads);
    assert_eq!(titles(&s), vec!["Fine"]);
    assert!(said["skipped"].as_u64().unwrap() >= 7);
    // A collection that isn't the account's, or is on a server the settings don't name, is not read.
    let mut elsewhere = read(vec![change("y.ics", "\"1\"", &event("y", 2, "Y"))], false, true, "t");
    elsewhere.href = "https://evil.example.net/cal/x/".into();
    apply(&mut s, id, None, vec![elsewhere]);
    assert_eq!(titles(&s), vec!["Fine"]);
    // Nor does a listing make a calendar of one.
    let mut l = listing();
    l.calendars.push(mailcraft_model::DavCalendar { href: "https://evil.example.net/cal/x/".into(), name: "Evil".into(), ..Default::default() });
    l.calendars.push(mailcraft_model::DavCalendar { href: "https://outlook.office365.com/cal/x/".into(), name: "MS".into(), ..Default::default() });
    apply(&mut s, id, Some(l), vec![]);
    assert!(s.mailbox.calendars.iter().all(|c| c.name != "Evil" && c.name != "MS"));
}

#[test]
fn a_calendar_the_server_stops_listing_goes_with_its_events_but_only_from_a_complete_listing() {
    let (mut s, id) = session();
    apply(&mut s, id, Some(listing()), vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], true, true, "")]);
    let mut empty = DavListing { complete: false, ..Default::default() };
    apply(&mut s, id, Some(empty.clone()), vec![]);
    assert_eq!(titles(&s), vec!["A"]);
    empty.complete = true;
    apply(&mut s, id, Some(empty), vec![]);
    assert!(titles(&s).is_empty() && !s.mailbox.calendars.iter().any(|c| c.origin.account == id));
    assert!(s.mailbox.account(id).unwrap().dav.as_ref().unwrap().collections.is_empty());
}

#[test]
fn a_calendar_of_the_mailbox_stays_where_new_events_go() {
    let (mut s, id) = session();
    let own = s.mailbox.calendars.len();
    apply(&mut s, id, Some(listing()), vec![]);
    assert_eq!(s.mailbox.calendars.len(), own + 1, "the mailbox's own calendar is not taken for the server's");
}

#[test]
fn putting_the_server_away_or_changing_it_takes_what_it_gave() {
    let (mut s, id) = session();
    apply(&mut s, id, Some(listing()), vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], true, true, "")]);
    let calendar = s.mailbox.calendars.iter().find(|c| c.origin.account == id).unwrap().id;
    let a = s.mailbox.events.values().find(|e| e.title == "A").unwrap().id;
    // Nothing goes up in this version, and that is what the owner is told, not that the server
    // refused them (its listing said the calendar may be written).
    let said = "The calendar \"Work\" is on a calendar server: this version reads it but can't send changes to it yet.";
    for (command, params) in [
        ("calendar.new", json!({"title": "Mine", "start": "2026-11-05T09:00", "calendar": calendar})),
        ("calendar.edit", json!({"id": a, "title": "Mine"})),
        ("calendar.delete", json!({"id": a})),
    ] {
        assert_eq!(s.run(command, &params).unwrap_err().to_string(), said, "{command}");
    }
    s.run("account.davSet", &json!({"id": id, "username": "grace"})).unwrap();
    assert!(!titles(&s).contains(&"A".to_string()));
    apply(&mut s, id, Some(listing()), vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], true, true, "")]);
    s.run("account.davOff", &json!({"id": id})).unwrap();
    assert!(titles(&s).is_empty());
}

#[test]
fn documents_are_kept_as_they_came_within_their_caps_and_go_with_their_event() {
    let dir = std::env::temp_dir().join(format!("mc-dav-raw-{}-{:?}", std::process::id(), std::thread::current().id()).replace(['(', ')'], ""));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("mailbox.mailcraft");
    let (mut s, id) = session();
    s.path = Some(file.clone());
    let a = format!("{COLLECTION}a.ics");
    let kept = event("a", 2, "A");
    let big = format!(
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:big\r\nDTSTART:20261103T100000Z\r\nDTEND:20261103T110000Z\r\nSUMMARY:Big\r\nDESCRIPTION:{}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "x".repeat(1_100_000)
    );
    apply(&mut s, id, Some(listing()), vec![read(vec![change("a.ics", "\"1\"", &kept), change("big.ics", "\"2\"", &big)], true, true, "")]);
    assert_eq!(crate::dav_raw::read(&file, id, &a).as_deref(), Some(kept.as_str()), "kept whole, as it came");
    assert!(!crate::dav_raw::has(&file, id, &format!("{COLLECTION}big.ics")), "over a megabyte: not kept");
    assert!(titles(&s).contains(&"Big".to_string()), "but shown, read-only, from its parsed fields");
    // A new version replaces it; a deletion takes it away.
    let newer = event("a", 2, "A again");
    apply(&mut s, id, None, vec![read(vec![change("a.ics", "\"3\"", &newer)], false, true, "t")]);
    assert_eq!(crate::dav_raw::read(&file, id, &a).as_deref(), Some(newer.as_str()));
    apply(&mut s, id, None, vec![read(vec![gone("a.ics")], false, true, "t")]);
    assert!(!crate::dav_raw::has(&file, id, &a));
    // Another server: all of it goes.
    apply(&mut s, id, None, vec![read(vec![change("a.ics", "\"4\"", &kept)], false, true, "t")]);
    s.run("account.davSet", &json!({"id": id, "username": "grace"})).unwrap();
    assert!(!crate::dav_raw::has(&file, id, &a));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_account_keeps_at_most_sixty_four_megabytes_and_lets_the_oldest_past_events_go_first() {
    let dir = std::env::temp_dir().join(format!("mc-dav-cap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("mailbox.mailcraft");
    let doc = "y".repeat(1_000_000);
    let hrefs: Vec<String> = (0..70).map(|i| format!("{COLLECTION}{i}.ics")).collect();
    // Document i is of an event that ended at time i.
    let ends: std::collections::HashMap<String, i64> = hrefs.iter().enumerate().map(|(i, h)| (crate::dav_raw::name_of(h), i as i64)).collect();
    let asked = std::cell::Cell::new(0);
    let mut store = crate::dav_raw::Store::open(&file, 1);
    for href in &hrefs {
        assert!(store.keep(href, &doc, ends[&crate::dav_raw::name_of(href)], &|| {
            asked.set(asked.get() + 1);
            ends.clone()
        }));
    }
    assert!(asked.get() < 70, "the events are looked at when there is no room, not at every document");
    let kept: Vec<bool> = hrefs.iter().map(|h| crate::dav_raw::has(&file, 1, h)).collect();
    let count = kept.iter().filter(|k| **k).count();
    assert!(count as u64 * 1_000_000 <= crate::dav_raw::MAX_ACCOUNT_RAW && count >= 55, "{count}");
    assert!(!kept[0] && kept[69], "the oldest went, the newest stayed");
    // Another account's budget is its own, and a document over the cap is never kept.
    assert!(!crate::dav_raw::Store::open(&file, 2).keep("x", &"z".repeat(crate::dav_raw::MAX_RAW + 1), 0, &|| ends.clone()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn junk_and_a_wrong_account_are_refused_and_change_nothing() {
    let (mut s, id) = session();
    apply(&mut s, id, Some(listing()), vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], true, true, "")]);
    for junk in [
        json!({}),
        json!({"result": 5}),
        json!({"result": {"problem": "bogus"}}),
        json!({"result": {"account": 999}}),
        json!({"result": {"account": id, "email": "other@example.org"}}),
    ] {
        assert!(s.run("sync.applyDav", &junk).is_err(), "{junk}");
    }
    assert_eq!(titles(&s), vec!["A"]);
    for hostile in [
        "BEGIN:VCALENDAR",
        "\u{0}\u{1}",
        &"BEGIN:VEVENT\r\n".repeat(5_000),
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART:99999999T999999\r\nEND:VEVENT\r\nEND:VCALENDAR",
    ] {
        let said = apply(&mut s, id, None, vec![read(vec![change("h.ics", "\"9\"", hostile)], false, true, "t")]);
        assert!(said["problem"] == "none", "{said}");
    }
    let mut bad = read(vec![], false, true, "t".repeat(5_000).as_str());
    bad.ctag = "c".repeat(5_000);
    apply(&mut s, id, None, vec![bad]);
    let state = &s.mailbox.account(id).unwrap().dav.as_ref().unwrap().collections[0];
    assert!(state.sync_token.is_empty() && state.ctag != "c".repeat(5_000));
}

#[test]
fn the_problem_and_the_wait_are_answered_and_the_wait_is_capped() {
    let (mut s, id) = session();
    let download = DavDownload {
        account: id,
        email: "ada@example.org".into(),
        server: server_of(&s, id),
        problem: DavProblem::Busy,
        wait: 99_999,
        ..Default::default()
    };
    let said = s.run("sync.applyDav", &json!({"result": download})).unwrap();
    assert_eq!((said["problem"].clone(), said["wait"].clone()), (json!("busy"), json!(600)));
}

fn graph_data(s: &mut Session, account: Id) -> (Id, Id) {
    use mailcraft_model::remote::{EventChange, EventChangeKind};
    use mailcraft_model::{Calendar, Event, Origin, Task};
    let calendar = s.mailbox.alloc_id();
    s.mailbox.calendars.push(Calendar {
        id: calendar,
        name: "Graph calendar".into(),
        origin: Origin { account, id: "AAMkGraphCalendar".into(), ..Default::default() },
        ..Default::default()
    });
    let event = s
        .mailbox
        .add_event(Event {
            title: "Graph meeting".into(),
            calendar,
            origin: Origin { account, id: "AAMkGraphEvent".into(), change_key: "ck".into(), ..Default::default() },
            ..Default::default()
        })
        .unwrap();
    s.mailbox.event_changes.push(EventChange {
        account,
        kind: EventChangeKind::Update,
        event,
        remote: "AAMkGraphEvent".into(),
        ..Default::default()
    });
    let task = s.mailbox.add_task(Task {
        title: "Graph task".into(),
        origin: Origin { account, id: "AAMkTask".into(), ..Default::default() },
        ..Default::default()
    });
    (event, task)
}

#[test]
fn putting_the_server_away_or_changing_it_leaves_what_microsoft_gave_the_account() {
    for how in ["off", "server"] {
        let (mut s, id) = session();
        apply(&mut s, id, Some(listing()), vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], true, true, "")]);
        let (graph_event, graph_task) = graph_data(&mut s, id);
        let calendars = s.mailbox.calendars.len();
        if how == "off" {
            s.run("account.davOff", &json!({"id": id})).unwrap();
        } else {
            s.run("account.davSet", &json!({"id": id, "username": "grace"})).unwrap();
        }
        assert!(!titles(&s).contains(&"A".to_string()), "{how}: the server's own goes");
        assert!(!s.mailbox.calendars.iter().any(|c| c.origin.id == COLLECTION), "{how}");
        assert_eq!(s.mailbox.calendars.len(), calendars - 1, "{how}: only its calendar");
        assert!(s.mailbox.calendars.iter().any(|c| c.origin.id == "AAMkGraphCalendar"), "{how}");
        assert!(s.mailbox.event(graph_event).is_some(), "{how}: Graph's event stays");
        assert!(s.mailbox.event_changes.iter().any(|c| c.event == graph_event), "{how}: and what waits to be sent for it");
        assert!(s.mailbox.tasks.contains_key(&graph_task), "{how}: and its to-dos");
    }
}

#[test]
fn each_account_with_a_server_has_its_turn() {
    let (mut s, first) = session();
    let second = s
        .run(
            "account.add",
            &json!({"email": "grace@example.org", "incoming": {"host": "imap2.example.org"}, "outgoing": {"host": "smtp2.example.org"}}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    s.run("account.davSet", &json!({"id": second, "calendarHomes": ["https://dav2.example.org/cal/grace/"]})).unwrap();
    let mut order: Vec<u64> = Vec::new();
    for _ in 0..4 {
        order.push(s.run("sync.davRequest", &json!({})).unwrap()["ask"]["account"].as_u64().unwrap());
    }
    assert_eq!(order[0], first);
    assert_eq!(order[1], second, "the one asked longest ago is next");
    assert_eq!(order[..2], order[2..]);
    // An account can still be named.
    assert_eq!(s.run("sync.davRequest", &json!({"account": second})).unwrap()["ask"]["account"], second);
    s.run("account.davOff", &json!({"id": first})).unwrap();
    assert_eq!(s.run("sync.davRequest", &json!({})).unwrap()["ask"]["account"], second);
}

#[test]
fn five_thousand_changes_look_at_the_documents_folder_once() {
    let dir = std::env::temp_dir().join(format!("mc-dav-many-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (mut s, id) = session();
    s.path = Some(dir.join("mailbox.mailcraft"));
    apply(&mut s, id, Some(listing()), vec![]);
    let changes: Vec<DavChange> =
        (0..5_000).map(|i| change(&format!("{i}.ics"), "\"1\"", &event(&format!("u{i}"), 2 + (i % 25) as u32, "E"))).collect();
    // (Work is counted, not timed: a disk's speed is not the code's.)
    let before = crate::dav_raw::LISTINGS.with(|n| n.get());
    let out = apply(&mut s, id, None, vec![read(changes, true, true, "t")]);
    assert_eq!(out["added"], 5_000);
    assert_eq!(crate::dav_raw::LISTINGS.with(|n| n.get()) - before, 1, "the folder is listed once for the read, not for each change");
    // (The writing is done on the documents' own thread: what is on disk is looked at once it is.)
    crate::dav_raw::settle();
    assert_eq!(std::fs::read_dir(crate::store::dav_dir(s.path.as_ref().unwrap()).join(id.to_string())).unwrap().count(), 5_000);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_oldest_events_of_this_run_go_first_when_the_documents_do_not_fit() {
    let dir = std::env::temp_dir().join(format!("mc-dav-evict-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("mailbox.mailcraft");
    let (mut s, id) = session();
    s.path = Some(file.clone());
    // 70 documents of almost a megabyte in one read: event i is in the year 2000 + i.
    let doc = |i: usize| {
        format!(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:u{i}\r\nDTSTART:{y}0601T100000Z\r\nDTEND:{y}0601T110000Z\r\nSUMMARY:E{i}\r\nDESCRIPTION:{}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            "x".repeat(990_000),
            y = 2000 + i
        )
    };
    let changes: Vec<DavChange> = (0..70).map(|i| change(&format!("{i}.ics"), "\"1\"", &doc(i))).collect();
    apply(&mut s, id, Some(listing()), vec![read(changes, true, true, "")]);
    let has = |i: usize| crate::dav_raw::has(&file, id, &format!("{COLLECTION}{i}.ics"));
    assert!(!has(0) && !has(1), "the oldest of what this read brought went");
    assert!(has(69) && has(68), "the newest stayed");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn removing_an_account_takes_its_kept_documents() {
    let dir = std::env::temp_dir().join(format!("mc-dav-remove-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("mailbox.mailcraft");
    let (mut s, id) = session();
    s.path = Some(file.clone());
    apply(&mut s, id, Some(listing()), vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], true, true, "")]);
    assert!(crate::dav_raw::has(&file, id, &format!("{COLLECTION}a.ics")));
    s.run(
        "account.add",
        &json!({"email": "grace@example.org", "incoming": {"host": "imap2.example.org"}, "outgoing": {"host": "smtp2.example.org"}}),
    )
    .unwrap();
    s.run("account.remove", &json!({"id": id})).unwrap();
    assert!(!crate::dav_raw::has(&file, id, &format!("{COLLECTION}a.ics")));
    crate::dav_raw::settle();
    assert!(!crate::store::dav_dir(&file).join(id.to_string()).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_mailbox_keeps_a_calendar_of_its_own_when_the_server_goes() {
    let (mut s, id) = session();
    apply(&mut s, id, Some(listing()), vec![]);
    // Only the server's calendar, and a copy of one on the web (which is nobody's to put events in).
    s.mailbox.calendars.retain(|c| c.origin.account == id);
    let copy = s.mailbox.alloc_id();
    s.mailbox.calendars.push(mailcraft_model::Calendar {
        id: copy,
        name: "Holidays".into(),
        subscription: Some(mailcraft_model::subscription::Subscription::default()),
        ..Default::default()
    });
    s.run("account.davOff", &json!({"id": id})).unwrap();
    assert!(
        s.mailbox.calendars.iter().any(|c| c.origin.is_local() && c.subscription.is_none()),
        "a calendar of the mailbox's own is left: {:?}",
        s.mailbox.calendars.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
}

#[test]
fn what_is_here_without_an_etag_is_named_in_what_is_asked_so_that_it_is_not_fetched_again() {
    let (mut s, id) = session();
    apply(&mut s, id, Some(listing()), vec![read(vec![change("a.ics", "", &event("a", 2, "A"))], true, true, "")]);
    let ask = s.run("sync.davRequest", &json!({})).unwrap();
    assert_eq!(ask["ask"]["collections"][0]["known"], json!([{"href": format!("{COLLECTION}a.ics"), "etag": ""}]));
}

#[test]
fn accounts_named_to_skip_are_passed_over() {
    let (mut s, first) = session();
    let second = s
        .run(
            "account.add",
            &json!({"email": "grace@example.org", "incoming": {"host": "imap2.example.org"}, "outgoing": {"host": "smtp2.example.org"}}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    s.run("account.davSet", &json!({"id": second, "calendarHomes": ["https://dav2.example.org/cal/grace/"]})).unwrap();
    for _ in 0..3 {
        assert_eq!(s.run("sync.davRequest", &json!({"skip": [first]})).unwrap()["ask"]["account"], second);
    }
    assert!(s.run("sync.davRequest", &json!({"skip": [first, second]})).is_err());
}

#[test]
fn a_document_older_than_everything_held_makes_no_room_and_is_not_kept() {
    let dir = std::env::temp_dir().join(format!("mc-dav-newest-first-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("mailbox.mailcraft");
    let doc = "y".repeat(1_000_000);
    let hrefs: Vec<String> = (0..70).map(|i| format!("{COLLECTION}{i}.ics")).collect();
    // Newest first: document 0 is of the event that ends last.
    let ends: std::collections::HashMap<String, i64> =
        hrefs.iter().enumerate().map(|(i, h)| (crate::dav_raw::name_of(h), 1_000 - i as i64)).collect();
    let mut store = crate::dav_raw::Store::open(&file, 1);
    let kept: Vec<bool> = hrefs.iter().map(|h| store.keep(h, &doc, ends[&crate::dav_raw::name_of(h)], &|| ends.clone())).collect();
    assert!(kept[0] && kept[10], "the newest are kept");
    assert!(!kept[69] && !crate::dav_raw::has(&file, 1, &hrefs[69]), "the oldest has no room made by throwing out newer ones");
    assert!(crate::dav_raw::has(&file, 1, &hrefs[0]), "and nothing newer was thrown out for it");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_document_is_let_go_of_whether_or_not_it_was_there_when_the_folder_was_listed() {
    let dir = std::env::temp_dir().join(format!("mc-dav-late-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("mailbox.mailcraft");
    let href = format!("{COLLECTION}a.ics");
    let mut store = crate::dav_raw::Store::open(&file, 1);
    // Written by another store after this one listed the folder.
    assert!(crate::dav_raw::Store::open(&file, 1).keep(&href, "doc", 0, &std::collections::HashMap::new));
    assert!(crate::dav_raw::has(&file, 1, &href));
    store.drop_one(&href);
    assert!(!crate::dav_raw::has(&file, 1, &href));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- the invariants, held by design (round 5) -------------------------------------------------

#[test]
fn i1_a_series_the_expansion_misjudges_is_never_removed_by_a_read_of_a_stretch() {
    // BYMONTH is not expanded here: read as daily it has days in the stretch, and the server (which
    // knows better) leaves it out of the stretch's answer. Nothing but the server's own word may
    // remove it.
    let (mut s, id) = session();
    s.clock.now = Some(1_793_000_000);
    let series = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:s\r\nDTSTART:20261101T100000Z\r\nDTEND:20261101T110000Z\r\nRRULE:FREQ=DAILY;BYMONTH=7;COUNT=3\r\nSUMMARY:Summer\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    apply(&mut s, id, Some(listing()), vec![read(vec![change("s.ics", "\"1\"", series)], true, true, "")]);
    // What a time-range read would have been: complete, not naming it. It is no listing of the
    // collection, so it says nothing of what is gone.
    apply(&mut s, id, None, vec![read(vec![], false, true, "")]);
    assert_eq!(titles(&s), vec!["Summer"]);
}

#[test]
fn i1_a_body_that_is_not_a_whole_calendar_leaves_the_events_as_they_were() {
    let (mut s, id) = session();
    apply(&mut s, id, Some(listing()), vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], true, true, "t1")]);
    for junk in ["BEGIN:VCALENDAR\r\n", "", "<html>Service Unavailable</html>", "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\n"] {
        let said = apply(&mut s, id, None, vec![read(vec![change("a.ics", "\"2\"", junk)], false, true, "t2")]);
        assert_eq!(said["removed"], 0, "{junk:?}");
        assert_eq!(titles(&s), vec!["A"], "{junk:?}");
    }
}

#[test]
fn i1_a_listing_longer_than_is_taken_in_removes_no_calendar() {
    let (mut s, id) = session();
    let mut first = listing();
    first.calendars[0].href = "https://dav.example.org/cal/ada/zz-last/".into();
    apply(&mut s, id, Some(first), vec![]);
    assert!(s.mailbox.calendars.iter().any(|c| c.origin.account == id));
    // The server lists more than are taken in, the held one last, and says it is complete.
    let mut long = DavListing { complete: true, capped: false, calendars: Vec::new() };
    for n in 0..mailcraft_model::DavServer::MAX_COLLECTIONS {
        long.calendars.push(mailcraft_model::DavCalendar { href: format!("https://dav.example.org/cal/ada/c{n:03}/"), ..Default::default() });
    }
    long.calendars.push(mailcraft_model::DavCalendar { href: "https://dav.example.org/cal/ada/zz-last/".into(), ..Default::default() });
    apply(&mut s, id, Some(long), vec![]);
    assert!(
        s.mailbox.account(id).unwrap().dav.as_ref().unwrap().collections.iter().any(|c| c.href.ends_with("zz-last/")),
        "a calendar past the cap was listed: it is not gone"
    );
}

#[test]
fn i2_the_token_does_not_move_past_changes_that_were_not_taken_in() {
    let (mut s, id) = session();
    apply(&mut s, id, Some(listing()), vec![read(vec![], true, true, "t1")]);
    let changes: Vec<DavChange> =
        (0..mailcraft_model::dav_down::MAX_CHANGES + 1).map(|n| change(&format!("{n}.ics"), "\"1\"", &event(&format!("u{n}"), 2, "E"))).collect();
    apply(&mut s, id, None, vec![read(changes, false, true, "t2")]);
    let state = &s.mailbox.account(id).unwrap().dav.as_ref().unwrap().collections[0];
    assert_eq!(state.sync_token, "t1", "the last change was never taken in: the token stays where it was");
}

#[test]
fn i2_a_download_read_for_other_settings_is_not_taken_in() {
    let (mut s, id) = session();
    s.run("account.davSet", &json!({"id": id, "username": "grace"})).unwrap();
    // A job that went out under the old user name comes back now.
    let stale = DavDownload { account: id, email: "ada@example.org".into(), listing: Some(listing()), ..Default::default() };
    let _ = s.run("sync.applyDav", &json!({"result": stale}));
    assert!(!s.mailbox.calendars.iter().any(|c| c.origin.account == id), "nothing of the old settings' read is taken in");
}

#[test]
fn i2_another_mailbox_starts_with_no_memory_of_when_calendars_were_listed() {
    let (mut s, id) = session();
    apply(&mut s, id, Some(listing()), vec![]);
    assert!(!s.dav_listed.is_empty());
    let other = s.mailbox.clone();
    s.replace_mailbox(other);
    assert!(s.dav_listed.is_empty() && s.dav_asked.is_empty());
}

#[test]
fn i2_what_is_here_is_named_even_while_a_token_is_kept_so_a_retry_skips_it() {
    let (mut s, id) = session();
    apply(&mut s, id, Some(listing()), vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], true, true, "t1")]);
    let ask = s.run("sync.davRequest", &json!({})).unwrap();
    assert_eq!(ask["ask"]["collections"][0]["syncToken"], "t1");
    assert_eq!(ask["ask"]["collections"][0]["known"], json!([{"href": format!("{COLLECTION}a.ics"), "etag": "\"1\""}]));
}

#[test]
fn i2_what_is_here_without_an_etag_is_read_again_once_a_day() {
    let (mut s, id) = session();
    s.clock.now = Some(1_793_000_000);
    apply(&mut s, id, Some(listing()), vec![read(vec![change("a.ics", "", &event("a", 2, "A"))], true, true, "")]);
    let known = |s: &mut Session| s.run("sync.davRequest", &json!({})).unwrap()["ask"]["collections"][0]["known"].clone();
    assert_eq!(known(&mut s), json!([{"href": format!("{COLLECTION}a.ics"), "etag": ""}]), "read a moment ago");
    s.clock.now = Some(1_793_000_000 + 86_400 + 1);
    assert_eq!(known(&mut s), json!([]), "a day on: it is not named, so a read fetches it again");
}

#[test]
fn i3_the_events_are_looked_at_once_for_a_whole_store_however_many_documents_are_refused() {
    let dir = std::env::temp_dir().join(format!("mc-dav-once-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("mailbox.mailcraft");
    let doc = "y".repeat(1_000_000);
    let hrefs: Vec<String> = (0..120).map(|i| format!("{COLLECTION}{i}.ics")).collect();
    // Newest first: from the cap on, every one that comes is older than all that is held.
    let ends: std::collections::HashMap<String, i64> =
        hrefs.iter().enumerate().map(|(i, h)| (crate::dav_raw::name_of(h), 1_000 - i as i64)).collect();
    let asked = std::cell::Cell::new(0);
    let mut store = crate::dav_raw::Store::open(&file, 1);
    for href in &hrefs {
        store.keep(href, &doc, ends[&crate::dav_raw::name_of(href)], &|| {
            asked.set(asked.get() + 1);
            ends.clone()
        });
    }
    assert_eq!(asked.get(), 1, "once, not at every document that finds no room");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- found in review ---------------------------------------------------------------------------

#[test]
fn r_a_resource_that_gives_no_event_is_named_in_what_is_asked_so_that_it_is_not_fetched_again() {
    let (mut s, id) = session();
    let todo = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:t\r\nSUMMARY:Task\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
    apply(&mut s, id, Some(listing()), vec![read(vec![change("t.ics", "\"7\"", todo), change("x.ics", "\"8\"", "not a calendar")], true, false, "")]);
    let ask = s.run("sync.davRequest", &json!({})).unwrap();
    let known = &ask["ask"]["collections"][0]["known"];
    for name in ["t.ics", "x.ics"] {
        assert!(known.as_array().unwrap().iter().any(|k| k["href"] == format!("{COLLECTION}{name}")), "{name}: {known}");
    }
    // Gone from a whole listing: forgotten.
    apply(&mut s, id, None, vec![read(vec![], true, true, "")]);
    let ask = s.run("sync.davRequest", &json!({})).unwrap();
    assert_eq!(ask["ask"]["collections"][0]["known"], json!([]));
}

#[test]
fn r_a_new_version_that_cannot_be_read_whole_removes_nothing_of_the_old() {
    let (mut s, id) = session();
    apply(&mut s, id, Some(listing()), vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], true, true, "")]);
    // A calendar with an event that doesn't parse: what is here stays.
    let broken = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nEND:VCALENDAR\r\n";
    apply(&mut s, id, None, vec![read(vec![change("a.ics", "\"2\"", broken)], false, true, "t")]);
    assert_eq!(titles(&s), vec!["A"]);
}

#[test]
fn r_a_read_that_stopped_short_with_no_token_leaves_the_token_where_it_was() {
    let (mut s, id) = session();
    apply(&mut s, id, Some(listing()), vec![read(vec![], false, true, "tok-1")]);
    apply(&mut s, id, None, vec![read(vec![], true, false, "")]);
    assert_eq!(s.mailbox.account(id).unwrap().dav.as_ref().unwrap().collections[0].sync_token, "tok-1");
}

#[test]
fn r_a_calendar_the_server_no_longer_lists_stays_while_something_in_it_waits_to_be_sent() {
    use mailcraft_model::remote::{EventChange, EventChangeKind};
    let (mut s, id) = session();
    apply(&mut s, id, Some(listing()), vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], true, true, "")]);
    let event = *s.mailbox.events.keys().next().unwrap();
    s.mailbox.event_changes.push(EventChange { account: id, kind: EventChangeKind::Update, event, ..Default::default() });
    let mut empty = listing();
    empty.calendars.clear();
    apply(&mut s, id, Some(empty), vec![]);
    assert_eq!(titles(&s), vec!["A"]);
}

#[test]
fn r_a_document_with_an_event_the_reader_could_not_make_out_removes_nothing_of_it() {
    let (mut s, id) = session();
    let two = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART:20261102T100000Z\r\nDTEND:20261102T110000Z\r\nSUMMARY:A\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:a\r\nRECURRENCE-ID:20261103T100000Z\r\nDTSTART:20261103T120000Z\r\nDTEND:20261103T130000Z\r\nSUMMARY:A moved\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    apply(&mut s, id, Some(listing()), vec![read(vec![change("a.ics", "\"1\"", two)], true, true, "")]);
    let before = titles(&s);
    // The second event is cut off before its end.
    let cut = two.replacen("SUMMARY:A moved\r\nEND:VEVENT\r\n", "SUMMARY:A moved\r\n", 1);
    apply(&mut s, id, None, vec![read(vec![change("a.ics", "\"2\"", &cut)], false, true, "t")]);
    assert_eq!(titles(&s), before);
}

#[test]
fn r_a_calendar_that_replaces_one_finds_room_when_the_calendars_are_at_their_cap() {
    let (mut s, id) = session();
    apply(&mut s, id, Some(listing()), vec![]);
    {
        let d = s.mailbox.account_mut(id).unwrap().dav.as_mut().unwrap();
        while d.collections.len() < mailcraft_model::DavServer::MAX_COLLECTIONS {
            let n = d.collections.len();
            d.collections
                .push(mailcraft_model::DavCollectionState { href: format!("https://dav.example.org/cal/ada/old{n}/"), ..Default::default() });
        }
    }
    let mut new = listing();
    new.calendars[0].href = "https://dav.example.org/cal/ada/new/".into();
    apply(&mut s, id, Some(new), vec![]);
    let d = s.mailbox.account(id).unwrap().dav.as_ref().unwrap();
    assert_eq!(d.collections.iter().map(|c| c.href.as_str()).collect::<Vec<_>>(), vec!["https://dav.example.org/cal/ada/new/"]);
}

// ---- found in review, round 6 ------------------------------------------------------------------

/// An event of the server's put straight into the mailbox, in the account's calendar.
fn held(s: &mut Session, id: Id, key: &str, description: String) -> Id {
    let calendar = s.mailbox.calendars.iter().find(|c| c.origin.account == id).unwrap().id;
    let origin = mailcraft_model::Origin { account: id, id: key.into(), change_key: "\"1\"".into(), ..Default::default() };
    s.mailbox.add_event(mailcraft_model::Event { calendar, title: key.into(), description, origin, ..Default::default() }).unwrap()
}

#[test]
fn r6_event_text_past_the_accounts_budget_is_not_taken_in_nothing_held_goes_and_it_is_said() {
    use mailcraft_model::dav_down::MAX_ACCOUNT_TEXT;
    let (mut s, id) = session();
    apply(&mut s, id, Some(listing()), vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], false, true, "tok-1")]);
    // What is held already comes to all but a few bytes of the budget.
    let filler = held(&mut s, id, &format!("{COLLECTION}big.ics"), String::new());
    let used: usize = s.mailbox.events.values().filter(|e| e.origin.account == id).map(|e| super::dav_sync::text_size(e)).sum();
    s.mailbox.event_mut(filler).unwrap().description = "x".repeat(MAX_ACCOUNT_TEXT - used - 10);
    let said = apply(&mut s, id, None, vec![read(vec![change("b.ics", "\"1\"", &event("b", 3, "B"))], false, true, "tok-2")]);
    assert_eq!((said["added"].clone(), said["problem"].clone()), (json!(0), json!("full")), "{said}");
    assert!(!titles(&s).contains(&"B".to_string()));
    assert_eq!(s.mailbox.account(id).unwrap().dav.as_ref().unwrap().collections[0].sync_token, "tok-1", "asked for again");
    // A new version of one held that would go past it: the one held stays as it was.
    let longer = event("a", 2, &"A".repeat(400));
    let said = apply(&mut s, id, None, vec![read(vec![change("a.ics", "\"2\"", &longer)], false, true, "tok-2")]);
    assert_eq!((said["updated"].clone(), said["problem"].clone()), (json!(0), json!("full")));
    assert!(titles(&s).contains(&"A".to_string()), "nothing held is let go of to make room");
    // Room made on the server's own word: then it comes in.
    let said = apply(&mut s, id, None, vec![read(vec![gone("big.ics"), change("b.ics", "\"1\"", &event("b", 3, "B"))], false, true, "tok-3")]);
    assert_eq!((said["added"].clone(), said["problem"].clone()), (json!(1), json!("none")));
}

#[test]
fn r6_a_read_asked_for_before_the_server_was_put_away_and_set_again_is_not_taken_in() {
    let (mut s, id) = session();
    let homes = json!({"id": id, "calendarHomes": ["https://dav.example.org/cal/ada/"]});
    let stale = |s: &mut Session| {
        let ask: mailcraft_model::DavAsk = serde_json::from_value(s.run("sync.davRequest", &json!({})).unwrap()["ask"].clone()).unwrap();
        DavDownload {
            account: id,
            email: ask.email,
            server: ask.server,
            generation: ask.generation,
            listing: Some(listing()),
            collections: vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], true, true, "")],
            ..Default::default()
        }
    };
    // Off and on again while the job is out.
    let job = stale(&mut s);
    s.run("account.davOff", &json!({"id": id})).unwrap();
    s.run("account.davSet", &homes).unwrap();
    assert!(s.run("sync.applyDav", &json!({"result": job})).is_err());
    assert!(titles(&s).is_empty() && !s.mailbox.calendars.iter().any(|c| c.origin.account == id));
    // To another server and back.
    let job = stale(&mut s);
    s.run("account.davSet", &json!({"id": id, "username": "grace"})).unwrap();
    s.run("account.davSet", &json!({"id": id, "username": ""})).unwrap();
    assert_eq!(job.server, server_of(&s, id), "the same settings again");
    assert!(s.run("sync.applyDav", &json!({"result": job})).is_err());
    // One asked for now is taken in.
    let job = stale(&mut s);
    s.run("sync.applyDav", &json!({"result": job})).unwrap();
    assert_eq!(titles(&s), vec!["A"]);
}

#[test]
fn r6_format_characters_are_stripped_and_every_text_is_capped() {
    let (mut s, id) = session();
    let categories: Vec<String> = (0..80).map(|i| format!("c{i}{}", "k".repeat(600))).collect();
    let doc = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:{}\u{202e}u\r\nDTSTART:20261102T100000Z\r\nSUMMARY:Pay\u{202e}fdp.exe\r\nLOCATION:Room\u{200b} 1\r\nDESCRIPTION:Note\u{2066}d\r\nCATEGORIES:{}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        "u".repeat(200),
        categories.join(",")
    );
    apply(&mut s, id, Some(listing()), vec![read(vec![change("a.ics", "\"1\"", &doc)], true, true, "")]);
    let e = s.mailbox.events.values().find(|e| e.origin.account == id).unwrap();
    for text in [&e.title, &e.location, &e.description, &e.uid] {
        assert!(!text.chars().any(mailcraft_names::is_format), "{text:?}");
    }
    assert_eq!(e.title, "Payfdp.exe");
    assert!(e.uid.chars().count() <= mailcraft_model::remote::MAX_KEY);
    assert!(e.categories.len() <= 50 && e.categories.iter().all(|c| c.chars().count() <= 500), "{}", e.categories.len());
}

#[test]
fn r6_a_new_version_that_could_not_be_read_is_named_by_its_new_etag_so_it_is_not_fetched_again() {
    let (mut s, id) = session();
    apply(&mut s, id, Some(listing()), vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], true, true, "")]);
    apply(&mut s, id, None, vec![read(vec![change("a.ics", "\"2\"", "not a calendar")], false, true, "")]);
    assert_eq!(titles(&s), vec!["A"], "what is here stays");
    let ask = s.run("sync.davRequest", &json!({})).unwrap();
    assert_eq!(ask["ask"]["collections"][0]["known"], json!([{"href": format!("{COLLECTION}a.ics"), "etag": "\"2\""}]));
}

#[test]
fn r6_a_calendar_the_mailbox_has_no_room_for_is_never_read_and_that_is_said() {
    let (mut s, id) = session();
    while s.mailbox.calendars.len() < 200 {
        let cid = s.mailbox.alloc_id();
        s.mailbox.calendars.push(mailcraft_model::Calendar { id: cid, name: format!("Mine {cid}"), ..Default::default() });
    }
    let said = apply(&mut s, id, Some(listing()), vec![]);
    assert_eq!(said["problem"], "full");
    assert!(!s.mailbox.calendars.iter().any(|c| c.origin.account == id));
    let ask = s.run("sync.davRequest", &json!({})).unwrap();
    assert_eq!((ask["ask"]["collections"].clone(), ask["ask"]["noRoom"].clone()), (json!([]), json!(true)), "nothing to read, and none to find");
}

#[test]
fn r6_a_calendar_no_longer_listed_and_a_whole_listing_keep_what_is_not_the_servers() {
    let (mut s, id) = session();
    apply(&mut s, id, Some(listing()), vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], true, true, "")]);
    // Something in the calendar without an address of the server's.
    let own = held(&mut s, id, "", String::new());
    s.mailbox.event_mut(own).unwrap().title = "Mine".into();
    apply(&mut s, id, None, vec![read(vec![], true, true, "")]);
    assert_eq!(titles(&s), vec!["Mine"], "a whole listing of the collection takes only what the server gave");
    let mut empty = listing();
    empty.calendars.clear();
    apply(&mut s, id, Some(empty), vec![]);
    assert_eq!(titles(&s), vec!["Mine"]);
    let calendar = s.mailbox.event(own).unwrap().calendar;
    let c = s.mailbox.calendar(calendar).unwrap();
    assert_eq!((c.origin.account, c.origin.id.as_str()), (0, ""), "the calendar stays, as the mailbox's own");
}

#[test]
fn r7_a_calendars_name_loses_its_format_characters() {
    let (mut s, id) = session();
    let mut named = listing();
    named.calendars[0].name = format!("Pay\u{202e}fdp.exe{}", "x".repeat(2_000));
    apply(&mut s, id, Some(named), vec![]);
    let c = s.mailbox.calendars.iter().find(|c| c.origin.account == id).unwrap();
    assert!(c.name.starts_with("Payfdp.exe") && !c.name.chars().any(mailcraft_names::is_format), "{:?}", c.name);
    assert!(c.name.chars().count() <= mailcraft_model::MAX_NAME);
    let state = &s.mailbox.account(id).unwrap().dav.as_ref().unwrap().collections[0];
    assert!(!state.name.chars().any(mailcraft_names::is_format));
}

#[test]
fn r7_a_whole_listing_that_empties_a_collection_keeps_no_token_so_the_next_run_looks_again() {
    let (mut s, id) = session();
    apply(
        &mut s,
        id,
        Some(listing()),
        vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A")), change("b.ics", "\"2\"", &event("b", 3, "B"))], true, true, "tok-1")],
    );
    let state = |s: &Session| {
        let c = &s.mailbox.account(id).unwrap().dav.as_ref().unwrap().collections[0];
        (c.sync_token.clone(), c.ctag.clone())
    };
    // A whole listing with nothing in it: believed, but the token and change tag stay.
    let mut empty = read(vec![], true, true, "tok-2");
    empty.ctag = "ctag-2".into();
    let said = apply(&mut s, id, None, vec![empty.clone()]);
    assert_eq!(said["removed"], 2);
    assert!(titles(&s).is_empty());
    assert_eq!(state(&s), ("tok-1".to_string(), "ctag-1".to_string()));
    // Said again at the next run: now nothing is held, and they are kept.
    apply(&mut s, id, None, vec![empty]);
    assert_eq!(state(&s), ("tok-2".to_string(), "ctag-2".to_string()));
}
