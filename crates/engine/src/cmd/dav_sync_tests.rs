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

fn read(changes: Vec<DavChange>, full: bool, complete: bool, token: &str) -> DavRead {
    let present = if full { changes.iter().filter(|c| !c.gone).map(|c| c.href.clone()).collect() } else { Vec::new() };
    DavRead {
        href: COLLECTION.into(),
        state: DavReadState::Read,
        full,
        complete,
        sync_token: token.into(),
        ctag: if complete { "ctag-1".into() } else { String::new() },
        changes,
        present,
        ..Default::default()
    }
}

fn apply(s: &mut Session, id: Id, listing: Option<DavListing>, reads: Vec<DavRead>) -> Value {
    let download = DavDownload { account: id, email: "ada@example.org".into(), listing, collections: reads, ..Default::default() };
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
    assert_eq!(ask["ask"]["collections"][0]["known"], json!([]), "a token needs nothing more");
    s.clock.offset = Some(0);
    assert!(ask["ask"]["to"].as_i64().unwrap() > ask["ask"]["from"].as_i64().unwrap() + 390 * 86_400);
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
fn a_deletion_is_believed_only_from_a_complete_listing() {
    let (mut s, id) = session();
    apply(
        &mut s,
        id,
        Some(listing()),
        vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A")), change("b.ics", "\"2\"", &event("b", 3, "B"))], true, true, "")],
    );
    // A full read that stopped short, or was capped, leaves absent events alone.
    apply(&mut s, id, None, vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], true, false, "")]);
    assert_eq!(titles(&s), vec!["A", "B"]);
    let mut capped = read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], true, true, "");
    capped.capped = true;
    apply(&mut s, id, None, vec![capped]);
    assert_eq!(titles(&s), vec!["A", "B"]);
    // A complete one removes what isn't in it, and the stale token isn't kept.
    let said = apply(&mut s, id, None, vec![read(vec![change("a.ics", "\"1\"", &event("a", 2, "A"))], true, true, "tok-9")]);
    assert_eq!(said["removed"], 1);
    assert_eq!(titles(&s), vec!["A"]);
    // A read of a stretch only removes what is in the stretch, and never a series.
    let (mut s, id) = session();
    let series = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:s\r\nDTSTART:20270101T100000Z\r\nDTEND:20270101T110000Z\r\nRRULE:FREQ=DAILY;COUNT=3\r\nSUMMARY:Series\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    apply(
        &mut s,
        id,
        Some(listing()),
        vec![read(
            vec![
                change("a.ics", "\"1\"", &event("a", 2, "A")),
                change("old.ics", "\"2\"", &event("old", 3, "Old")),
                change("s.ics", "\"3\"", series),
            ],
            true,
            true,
            "",
        )],
    );
    let now = s.now();
    let mut query = read(vec![], true, true, "");
    query.window = Some((now - 86_400, now + 86_400));
    apply(&mut s, id, None, vec![query]);
    assert_eq!(titles(&s).len(), 3, "events outside the stretch and a series stay");
    let mut query = read(vec![], true, true, "");
    query.window = Some((0, i64::MAX / 2));
    apply(&mut s, id, None, vec![query]);
    assert_eq!(titles(&s), vec!["Series"]);
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
    let made = s.run("calendar.newEvent", &json!({"title": "Mine", "start": "2026-11-05T09:00", "end": "2026-11-05T10:00"}));
    s.run("account.davSet", &json!({"id": id, "username": "grace"})).unwrap();
    assert!(!titles(&s).contains(&"A".to_string()), "{made:?}");
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
    for href in &hrefs {
        assert!(crate::dav_raw::keep(&file, 1, href, &doc, &ends));
    }
    let kept: Vec<bool> = hrefs.iter().map(|h| crate::dav_raw::has(&file, 1, h)).collect();
    let count = kept.iter().filter(|k| **k).count();
    assert!(count as u64 * 1_000_000 <= crate::dav_raw::MAX_ACCOUNT_RAW && count >= 60, "{count}");
    assert!(!kept[0] && kept[69], "the oldest went, the newest stayed");
    // Another account's budget is its own, and a document over the cap is never kept.
    assert!(!crate::dav_raw::keep(&file, 2, "x", &"z".repeat(crate::dav_raw::MAX_RAW + 1), &ends));
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
    let download = DavDownload { account: id, email: "ada@example.org".into(), problem: DavProblem::Busy, wait: 99_999, ..Default::default() };
    let said = s.run("sync.applyDav", &json!({"result": download})).unwrap();
    assert_eq!((said["problem"].clone(), said["wait"].clone()), (json!("busy"), json!(600)));
}
