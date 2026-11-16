// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use std::sync::Arc;

use mailcraft_time::{Date, DateTime, Zone};

use super::offset_of;
use crate::{Clock, Session, sample};

fn session(zone: Zone, fallback: bool, pinned: Option<i32>) -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 23, 30, 0).unwrap().to_timestamp(0);
    let mut s = Session::with_clock(sample::mailbox(now, 0), Clock { now: Some(now), offset: pinned, zone: Some(zone) });
    if fallback {
        s.fallback_offset = Some(Arc::new(|_| 120));
    }
    s
}

#[test]
fn calendar_conversion_uses_the_phones_offset_where_the_system_names_no_zone() {
    let s = session(Zone::LOCAL, true, None);
    let wall = DateTime::new(Date::new(2026, 10, 15).unwrap(), 9, 0, 0).unwrap();
    assert_eq!(offset_of(&s)(0), 120);
    assert_eq!(s.offset_at(0), 120);
    assert_eq!(s.instant_of(wall), wall.to_timestamp(120));
    assert_eq!(s.wall_at(wall.to_timestamp(120)), wall);
}

#[test]
fn a_named_or_pinned_zone_goes_before_the_phones_offset() {
    let named = session(Zone::named("America/New_York").unwrap(), true, None);
    assert_eq!(offset_of(&named)(0), named.zone().offset_at(0));
    let pinned = session(Zone::LOCAL, true, Some(-60));
    assert_eq!(offset_of(&pinned)(0), -60);
    let wall = DateTime::new(Date::new(2026, 10, 15).unwrap(), 9, 0, 0).unwrap();
    assert_eq!(pinned.instant_of(wall), wall.to_timestamp(-60));
    // No fallback: the system's own (unnamed) zone as before.
    let bare = session(Zone::LOCAL, false, None);
    assert_eq!(offset_of(&bare)(0), Zone::LOCAL.offset_at(0));
}
