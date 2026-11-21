// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use std::sync::atomic::Ordering;

use mailcraft_engine::{Clock, Session, sample};
use mailcraft_time::{Date, DateTime, Zone};

use crate::Core;
use crate::fake_tests::{FakePlatform, block_on};

/// 23:30 UTC on 2026-10-14.
fn late() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 23, 30, 0).unwrap().to_timestamp(0)
}

fn today_on(platform: std::sync::Arc<FakePlatform>, zone: Option<Zone>, chosen: &'static str) -> Date {
    let core = Core::start(platform, move || {
        let clock = Clock { now: Some(late()), offset: None, zone };
        let mut session = Session::with_clock(sample::mailbox(late(), 0), clock);
        session.prefs.time_zone = chosen.to_string();
        session
    })
    .unwrap();
    block_on(core.call(|s| Ok(s.today()))).unwrap()
}

#[test]
fn the_phones_offset_picks_the_day_when_the_system_names_no_zone() {
    let platform = FakePlatform::new();
    platform.offset.store(120, Ordering::Relaxed);
    // The system's zone is the unnamed one: 23:30 UTC is already the 15th at +02:00.
    assert_eq!(today_on(platform, Some(Zone::LOCAL), ""), Date::new(2026, 10, 15).unwrap());
}

#[test]
fn a_zone_the_owner_chose_goes_before_the_phones_offset() {
    let platform = FakePlatform::new();
    platform.offset.store(120, Ordering::Relaxed);
    assert_eq!(today_on(platform, Some(Zone::LOCAL), "UTC"), Date::new(2026, 10, 14).unwrap());
}

#[test]
fn a_zone_the_system_names_goes_before_the_phones_offset() {
    let platform = FakePlatform::new();
    platform.offset.store(120, Ordering::Relaxed);
    let berlinless = Zone::named("America/New_York").unwrap();
    assert_eq!(today_on(platform, Some(berlinless), ""), Date::new(2026, 10, 14).unwrap());
}

#[test]
fn the_calendar_starts_on_the_phones_day_when_the_system_names_no_zone() {
    let platform = FakePlatform::new();
    platform.offset.store(120, Ordering::Relaxed);
    let core = Core::start(platform, || {
        let clock = Clock { now: Some(late()), offset: None, zone: Some(Zone::LOCAL) };
        Session::with_clock(sample::mailbox(late(), 0), clock)
    })
    .unwrap();
    let (today, shown) = block_on(core.call(|s| Ok((s.today(), s.view.date)))).unwrap();
    assert_eq!((today, shown), (Date::new(2026, 10, 15).unwrap(), Date::new(2026, 10, 15).unwrap()));
}

#[test]
fn a_date_the_session_was_made_with_is_kept() {
    let platform = FakePlatform::new();
    platform.offset.store(120, Ordering::Relaxed);
    let chosen = Date::new(2026, 1, 2).unwrap();
    let core = Core::start(platform, move || {
        let clock = Clock { now: Some(late()), offset: None, zone: Some(Zone::LOCAL) };
        let mut s = Session::with_clock(sample::mailbox(late(), 0), clock);
        s.view.date = chosen;
        s
    })
    .unwrap();
    assert_eq!(block_on(core.call(|s| Ok(s.view.date))).unwrap(), chosen);
}

#[test]
fn a_zone_name_the_phone_gives_goes_before_its_offset() {
    let platform = FakePlatform::new();
    platform.offset.store(120, Ordering::Relaxed);
    *platform.zone.lock().unwrap() = Some("America/New_York".to_string());
    // 23:30 UTC is still the 14th in New York, whatever the offset the phone also says.
    assert_eq!(today_on(platform, Some(Zone::LOCAL), ""), Date::new(2026, 10, 14).unwrap());
}

#[test]
fn a_zone_name_that_is_not_known_leaves_the_phones_offset_in_charge() {
    for name in ["Mars/Olympus_Mons", "", "x".repeat(10_000).as_str(), "\u{0}\n"] {
        let platform = FakePlatform::new();
        platform.offset.store(120, Ordering::Relaxed);
        *platform.zone.lock().unwrap() = Some(name.to_string());
        assert_eq!(today_on(platform, Some(Zone::LOCAL), ""), Date::new(2026, 10, 15).unwrap(), "{name:?}");
    }
}

#[test]
fn a_mailbox_stamped_with_another_zone_is_rebased_to_the_phones_named_zone() {
    let platform = FakePlatform::new();
    *platform.zone.lock().unwrap() = Some("America/New_York".to_string());
    let core = Core::start(platform, || {
        let mut mailbox = sample::mailbox(late(), 0);
        mailbox.zone = "Europe/Berlin".to_string();
        let clock = Clock { now: Some(late()), offset: None, zone: Some(Zone::LOCAL) };
        Session::with_clock(mailbox, clock)
    })
    .unwrap();
    let stamped = block_on(core.call(|s| Ok(s.mailbox.zone.clone()))).unwrap();
    assert_eq!(stamped, "America/New_York");
}

#[test]
fn a_zone_name_that_moves_the_day_moves_the_calendars_default_date() {
    let platform = FakePlatform::new();
    *platform.zone.lock().unwrap() = Some("Asia/Tokyo".to_string());
    let core = Core::start(platform, || {
        let clock = Clock { now: Some(late()), offset: None, zone: Some(Zone::LOCAL) };
        Session::with_clock(sample::mailbox(late(), 0), clock)
    })
    .unwrap();
    let (today, shown) = block_on(core.call(|s| Ok((s.today(), s.view.date)))).unwrap();
    // 23:30 UTC is already the 15th at +09:00, and the calendar opens on it.
    assert_eq!((today, shown), (Date::new(2026, 10, 15).unwrap(), Date::new(2026, 10, 15).unwrap()));
}
