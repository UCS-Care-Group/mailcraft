//! The window in every language: each module, File page and dialog lays out in each catalog's
//! language (and right to left in Arabic and Hebrew) without panicking, in a comfortable window
//! and in one far too small. With it, a look at what the interface asked to translate: text that
//! reached `tl!` without being among the keys read from the source is reported (it is data, such
//! as a folder's name, or a string still to be marked with `tn!`).

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::Lang;
use crate::tests::frames;
use crate::{MailApp, Services};
use mailcraft_engine::Session;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn app(language: &str) -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.execute("ui.language", json!({"value": language})).unwrap();
    app
}

fn screens() -> Vec<(&'static str, Vec<(&'static str, Value)>)> {
    vec![
        ("mail", vec![("view.mail", json!({})), ("mail.next", json!({}))]),
        ("mail, classic ribbon", vec![("ui.ribbonMode", json!({"value": "classic"})), ("mail.next", json!({}))]),
        ("writing", vec![("mail.next", json!({})), ("mail.replyAll", json!({}))]),
        ("calendar week", vec![("view.week", json!({}))]),
        ("calendar month", vec![("view.month", json!({}))]),
        ("people", vec![("view.people", json!({}))]),
        ("tasks", vec![("view.tasks", json!({})), ("view.taskFilter", json!({"value": "all"}))]),
        ("file: info", vec![("ui.backstage", json!({"value": true, "page": "info"}))]),
        ("file: options", vec![("ui.backstage", json!({"value": true, "page": "options"}))]),
        ("file: print", vec![("ui.backstage", json!({"value": true, "page": "print"}))]),
        ("dialog: event", vec![("ui.dialog", json!({"name": "event"}))]),
        ("dialog: meeting", vec![("ui.dialog", json!({"name": "meeting"}))]),
        ("dialog: contact", vec![("ui.dialog", json!({"name": "contact"}))]),
        ("dialog: task", vec![("ui.dialog", json!({"name": "task"}))]),
        ("dialog: rules", vec![("ui.dialog", json!({"name": "rules"}))]),
        ("dialog: account", vec![("ui.dialog", json!({"name": "account"}))]),
        ("dialog: commands", vec![("ui.dialog", json!({"name": "commands"}))]),
    ]
}

#[test]
fn every_language_lays_out_without_panicking() {
    let languages: Vec<&str> = std::iter::once("en").chain(std::iter::once("pseudo")).chain(Lang::TRANSLATED.iter().map(|l| l.code())).collect();
    for language in languages {
        for size in [(1280.0, 800.0), (360.0, 240.0)] {
            for (what, steps) in screens() {
                let mut a = app(language);
                for (id, params) in steps {
                    a.execute(id, params.clone()).unwrap_or_else(|e| panic!("{language}: {what}: {id}: {e}"));
                }
                let a = frames(a, size, 3);
                assert_eq!(super::current().code(), Lang::from_code(language).map_or("en", Lang::code), "{what}");
                drop(a);
            }
        }
    }
    super::set_current(Lang::English);
}

#[test]
fn what_the_window_asks_to_translate_is_known() {
    let keys: BTreeSet<String> = super::keys::all().into_iter().collect();
    super::start_recording();
    for (_, steps) in screens() {
        let mut a = app("en");
        for (id, params) in steps {
            let _ = a.execute(id, params.clone());
        }
        let _ = frames(a, (1280.0, 800.0), 3);
    }
    let asked = super::stop_recording();
    let unknown: Vec<&String> = asked.iter().filter(|s| !keys.contains(*s)).collect();
    let share = (asked.len() - unknown.len()) * 100 / asked.len().max(1);
    println!("{} strings asked for, {share}% of them known keys; not among the keys (data, or to be marked):", asked.len());
    for s in &unknown {
        println!("  {s:?}");
    }
    // Most of what the window asks for is text of its own.
    assert!(share >= 80, "only {share}% of what was asked for is among the keys");
}

#[test]
fn the_language_setting_is_checked_and_saved() {
    let mut a = app("de");
    assert_eq!(a.ui.language, "de");
    assert_eq!(super::current(), Lang::German);
    // German writes the 24-hour clock and starts the week on Monday, where the owner didn't
    // choose otherwise.
    assert!(!a.session.prefs.twelve_hour);
    assert_eq!(a.session.prefs.week_start, mailcraft_time::Weekday::Mon);
    let answer = a.execute("ui.language", json!({"value": "he"})).unwrap();
    assert_eq!(answer["rtl"], true);
    assert_eq!(answer["locale"], "he-IL");
    assert!(super::rtl());
    // Back to English: the clock the locale gave goes back too...
    a.execute("ui.language", json!({"value": "en"})).unwrap();
    assert!(a.session.prefs.twelve_hour);
    assert_eq!(a.session.prefs.week_start, mailcraft_time::Weekday::Sun);
    // ...but a clock the owner chose stays through a change of language.
    a.execute("file.options", json!({"twelveHour": false})).unwrap();
    a.execute("ui.language", json!({"value": "fr"})).unwrap();
    a.execute("ui.language", json!({"value": "en"})).unwrap();
    assert!(!a.session.prefs.twelve_hour);
    for junk in [json!({"value": "klingon"}), json!({"value": 3}), json!({}), json!({"value": "x".repeat(10_000)})] {
        assert!(a.execute("ui.language", junk).is_err());
    }
    assert_eq!(a.ui.language, "en");
    // It is saved with the window's settings.
    let saved = serde_json::to_value(&a.ui).unwrap();
    assert_eq!(saved["language"], "en");
    super::set_current(Lang::English);
}
