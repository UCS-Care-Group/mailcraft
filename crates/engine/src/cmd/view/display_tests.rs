// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The list's ready-made views, tighter spacing, the zoom of the message being read and the
//! searches run lately.

use serde_json::{Value, json};

use super::{ListView, ZOOM_STEPS, remember_search, zoom_step};
use crate::{Pane, Prefs, Session};

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

#[test]
fn change_view_sets_the_list_and_the_reading_pane() {
    let mut s = Session::empty();
    assert_eq!(run(&mut s, "view.changeView", json!({}))["value"], "compact", "as it comes");
    let r = run(&mut s, "view.changeView", json!({"value": "preview"}));
    assert_eq!(r["value"], "preview");
    assert_eq!((s.prefs.one_line, s.prefs.preview_lines, s.view.reading_pane), (false, 3, Pane::Off));
    run(&mut s, "view.changeView", json!({"value": "single"}));
    assert_eq!((s.prefs.one_line, s.view.reading_pane), (true, Pane::Right));
    assert_eq!(ListView::of(&s), Some(ListView::Single));
    run(&mut s, "view.changeView", json!({"value": "compact"}));
    assert_eq!((s.prefs.one_line, s.prefs.preview_lines, s.view.reading_pane), (false, 1, Pane::Right));
    // Changed by hand, it is none of them.
    run(&mut s, "view.readingPane", json!({"value": "bottom"}));
    assert_eq!(run(&mut s, "view.changeView", json!({}))["value"], "custom");
    assert!(s.run("view.changeView", &json!({"value": "tiles"})).is_err());
    assert!(s.run("view.changeView", &json!({"value": 3})).is_err());
}

#[test]
fn tighter_spacing_toggles_and_is_kept_in_the_settings() {
    let mut s = Session::empty();
    assert!(!s.prefs.tight_spacing);
    assert_eq!(run(&mut s, "view.density", json!({}))["value"], true);
    assert_eq!(run(&mut s, "view.density", json!({"value": true}))["value"], true);
    let saved = serde_json::to_value(&s.prefs).unwrap();
    assert_eq!(saved["tightSpacing"], true);
    assert!(serde_json::from_value::<Prefs>(saved).unwrap().tight_spacing);
    assert_eq!(run(&mut s, "view.density", json!({}))["value"], false);
    assert!(serde_json::to_value(&s.prefs).unwrap().get("tightSpacing").is_none(), "off is not written");
}

#[test]
fn zoom_steps_and_stays_in_range() {
    let mut s = Session::empty();
    assert_eq!(run(&mut s, "view.zoom", json!({}))["value"], 100);
    assert_eq!(run(&mut s, "view.zoomIn", json!({}))["value"], 110);
    assert_eq!(run(&mut s, "view.zoom", json!({"step": "in"}))["value"], 125);
    assert_eq!(run(&mut s, "view.zoomOut", json!({}))["value"], 110);
    assert_eq!(run(&mut s, "view.zoomReset", json!({}))["value"], 100);
    assert_eq!(run(&mut s, "view.zoom", json!({"value": 1e9}))["value"], 300);
    assert_eq!(run(&mut s, "view.zoomIn", json!({}))["value"], 300, "no further than the most");
    assert_eq!(run(&mut s, "view.zoom", json!({"value": -4}))["value"], 50);
    assert_eq!(run(&mut s, "view.zoomOut", json!({}))["value"], 50, "no less than the least");
    assert_eq!(run(&mut s, "view.zoom", json!({"value": 133.4}))["value"], 133);
    assert_eq!(zoom_step(133, true), 150);
    assert_eq!(zoom_step(133, false), 125);
    assert!(s.run("view.zoom", &json!({"value": "big"})).is_err());
    assert!(s.run("view.zoom", &json!({"step": "sideways"})).is_err());
    // A settings file that says anything reads in range.
    s.prefs.zoom = 9000;
    assert_eq!(s.prefs.zoom_percent(), 300);
    s.prefs.zoom = 0;
    assert_eq!(run(&mut s, "view.zoom", json!({}))["value"], 50);
    assert!(ZOOM_STEPS.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(ZOOM_STEPS.first(), Some(&Prefs::ZOOM_MIN));
    assert_eq!(ZOOM_STEPS.last(), Some(&Prefs::ZOOM_MAX));
}

#[test]
fn recent_searches_are_the_latest_ten_once_each() {
    let mut s = Session::empty();
    assert_eq!(run(&mut s, "view.recentSearches", json!({}))["searches"], json!([]));
    for n in 0..14 {
        run(&mut s, "view.recentSearches", json!({"remember": format!("budget {n}")}));
    }
    let list = run(&mut s, "view.recentSearches", json!({"remember": "  budget 5\n"}));
    let list: Vec<&str> = list["searches"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(list.len(), Prefs::RECENT_SEARCHES);
    assert_eq!(list.first(), Some(&"budget 5"), "run again, it moves up");
    assert_eq!(list.iter().filter(|q| **q == "budget 5").count(), 1);
    assert_eq!(list.get(1), Some(&"budget 13"));
    // Nothing empty, nothing endless.
    remember_search(&mut s.prefs, " \u{0} ");
    assert_eq!(s.prefs.recent_searches().next(), Some("budget 5"));
    remember_search(&mut s.prefs, &"é".repeat(5000));
    assert_eq!(s.prefs.recent_searches().next().map(|q| q.chars().count()), Some(Prefs::RECENT_SEARCH_CHARS));
    assert_eq!(run(&mut s, "view.recentSearches", json!({"clear": true}))["searches"], json!([]));
    assert!(s.run("view.recentSearches", &json!({"remember": 7})).is_err());
}

#[test]
fn recent_searches_read_from_a_hostile_settings_file_stay_bounded() {
    let mut s = Session::empty();
    s.prefs.recent_searches = (0..1000).map(|n| if n % 2 == 0 { String::new() } else { "x".repeat(100_000) }).collect();
    let list: Vec<&str> = s.prefs.recent_searches().collect();
    assert_eq!(list.len(), Prefs::RECENT_SEARCHES);
    assert!(list.iter().all(|q| q.chars().count() == Prefs::RECENT_SEARCH_CHARS));
    let offered = crate::cmd::view::search_suggestions(&s, "", 0);
    assert_eq!(offered.len(), Prefs::RECENT_SEARCHES);
}

#[test]
fn an_empty_search_box_offers_the_recent_searches() {
    let mut s = Session::empty();
    assert!(crate::cmd::view::search_suggestions(&s, "", 0).is_empty());
    run(&mut s, "view.recentSearches", json!({"remember": "from:ana"}));
    run(&mut s, "view.recentSearches", json!({"remember": "budget"}));
    let offered = run(&mut s, "view.searchSuggestions", json!({"query": "  "}));
    assert_eq!(offered["suggestions"][0]["name"], "budget");
    assert_eq!(offered["suggestions"][0]["text"], "budget");
    assert_eq!(
        (offered["suggestions"][0]["start"].as_u64(), offered["suggestions"][0]["end"].as_u64()),
        (Some(0), Some(2)),
        "in place of what is there"
    );
    assert_eq!(offered["suggestions"][1]["name"], "from:ana");
    // Something typed: the recent ones are not what is offered.
    let typed = crate::cmd::view::search_suggestions(&s, "bud", 3);
    assert!(typed.iter().all(|x| x.text != "budget"));
}
