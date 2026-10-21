// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a command keeps of the messages it brings content into and lets go of again (#173).

use std::sync::Mutex;

use serde_json::{Value, json};

use super::*;
use crate::{CmdResult, CommandSpec};

/// How many swaps the last `test.swapMany` held when it was done with its loads.
static HELD: Mutex<usize> = Mutex::new(0);

const PASSES: usize = 100;

/// Puts message `id` in place `PASSES` times over, as a search reading every body does, changes
/// message `other` for real, then ends as `how` says: `ok`, `fail` or `panic`.
fn swap_many(s: &mut Session, p: &Value) -> CmdResult {
    let id = p["id"].as_u64().unwrap();
    let other = p["other"].as_u64().unwrap();
    for i in 0..PASSES {
        let mut m = (**s.mailbox.messages.get(&id).unwrap()).clone();
        m.body.text = format!("pass {i}");
        s.put_message(id, m);
    }
    *HELD.lock().unwrap() = s.swaps.as_ref().map_or(0, Swaps::len);
    let mut o = (**s.mailbox.messages.get(&other).unwrap()).clone();
    o.read = !o.read;
    s.mailbox.messages.insert(other, Arc::new(o));
    match p["how"].as_str() {
        Some("ok") => Ok(json!({})),
        Some("fail") => Err(CmdError::Failed("asked to".into())),
        _ => panic!("asked to"),
    }
}

#[test]
fn putting_one_message_in_place_many_times_keeps_one_swap_and_a_failure_still_goes_back() {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    let mut s = Session::at(crate::sample::mailbox(now, 0), now, 0);
    let mut specs = crate::cmd::registry().all().to_vec();
    specs.push(CommandSpec::new("test.swapMany", "Swap Many", "Test", swap_many));
    s.registry = Arc::new(Registry::new(specs));
    let mut ids = s.mailbox.messages.keys().copied();
    let (id, other) = (ids.next().unwrap(), ids.next().unwrap());
    let last = format!("pass {}", PASSES - 1);

    for how in ["fail", "panic"] {
        let read = s.mailbox.message(other).unwrap().read;
        assert!(s.run("test.swapMany", &json!({"id": id, "other": other, "how": how})).is_err());
        assert_eq!(*HELD.lock().unwrap(), 1, "{how}");
        // The real change is gone; the content brought in stays, as it was last put.
        assert_eq!(s.mailbox.message(other).unwrap().read, read, "{how}");
        assert_eq!(s.mailbox.message(id).unwrap().body.text, last, "{how}");
        assert!(!s.can_undo(), "{how}");
        assert!(!s.dirty, "{how}");
    }

    let read = s.mailbox.message(other).unwrap().read;
    s.run("test.swapMany", &json!({"id": id, "other": other, "how": "ok"})).unwrap();
    assert_eq!(*HELD.lock().unwrap(), 1);
    assert_ne!(s.mailbox.message(other).unwrap().read, read);
    // Undo takes back the change and keeps the content as the command left it.
    assert!(s.undo());
    assert_eq!(s.mailbox.message(other).unwrap().read, read);
    assert_eq!(s.mailbox.message(id).unwrap().body.text, last);
}

#[test]
fn the_interface_language_is_the_hosts_and_never_saved() {
    let mut s = Session::empty();
    assert_eq!(s.interface_language, mailcraft_i18n::Lang::English);
    s.interface_language = mailcraft_i18n::Lang::Pseudo;
    let saved = serde_json::to_string(&s.mailbox).unwrap();
    assert!(!saved.contains("pseudo"), "the mailbox file says nothing of the window's language");
}

/// The clock and the first day of the week follow the interface language until the owner
/// chooses them; what they chose, and settings saved before, stay as they were.
#[test]
fn the_clock_and_the_week_follow_the_language_unless_chosen() {
    use mailcraft_i18n::Lang;
    use mailcraft_time::Weekday;
    let mut fresh = Prefs::default();
    fresh.follow_language(Lang::English);
    assert_eq!((fresh.twelve_hour, fresh.week_start), (true, Weekday::Sun), "English as it always was");
    fresh.follow_language(Lang::French);
    assert_eq!((fresh.twelve_hour, fresh.week_start), (false, Weekday::Mon), "French's own");
    fresh.follow_language(Lang::English);
    assert_eq!((fresh.twelve_hour, fresh.week_start), (true, Weekday::Sun), "and back");
    // Saved before the choice was kept: what differs from the old defaults was chosen.
    let mut old: Prefs = serde_json::from_value(json!({"twelveHour": false})).unwrap();
    old.follow_language(Lang::French);
    old.follow_language(Lang::English);
    assert_eq!((old.twelve_hour, old.week_start), (false, Weekday::Sun));
    let mut old: Prefs = serde_json::from_value(json!({"weekStart": "Mon"})).unwrap();
    old.follow_language(Lang::English);
    assert_eq!((old.twelve_hour, old.week_start), (true, Weekday::Mon));
    // Chosen in Options: kept whatever the language, and saved.
    let mut s = Session::empty();
    s.run("file.options", &json!({"twelveHour": true, "weekStart": "sun"})).unwrap();
    s.prefs.follow_language(Lang::French);
    assert_eq!((s.prefs.twelve_hour, s.prefs.week_start), (true, Weekday::Sun));
    let saved: Prefs = serde_json::from_value(serde_json::to_value(&s.prefs).unwrap()).unwrap();
    assert_eq!(saved, s.prefs);
}
