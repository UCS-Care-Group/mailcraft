// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Advanced Search's panel under the search box (#329).

use egui::{Key, PointerButton};
use egui_kittest::kittest::Queryable;
use serde_json::json;

use super::*;
use crate::people_view::testing::{self, Window};

fn window() -> Window {
    let mut w = testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.run_steps(2);
    w
}

fn open(w: &mut Window) {
    w.state_mut().execute("ui.advancedSearch", json!({"open": true})).unwrap();
    w.run_steps(3);
    assert!(w.state().hit.rect("advancedSearch", 0).is_some(), "the panel is drawn");
}

fn panel(w: &mut Window) -> &mut Panel {
    w.state_mut().advanced_search.as_mut().unwrap()
}

fn is_open(w: &Window) -> bool {
    w.state().advanced_search.is_some()
}

fn search(w: &Window) -> String {
    w.state().session.view.search.clone()
}

fn set_search(w: &mut Window, q: &str) {
    w.state_mut().execute("view.search", json!({"query": q})).unwrap();
    w.run_steps(3);
}

fn click_at(w: &mut Window, kind: &str) {
    let r = w.state().hit.rect(kind, 0).unwrap();
    testing::click(w, r.center(), PointerButton::Primary, 1);
}

#[test]
fn the_button_opens_and_closes_the_panel() {
    let mut w = window();
    click_at(&mut w, "advancedSearchButton");
    assert!(is_open(&w));
    assert!(w.state().hit.rect("advancedSearch", 0).is_some());
    click_at(&mut w, "advancedSearchButton");
    assert!(!is_open(&w));
}

#[test]
fn search_writes_the_box_sets_the_scope_and_closes_the_panel() {
    let mut w = window();
    set_search(&mut w, "lunch FROM: ada");
    open(&mut w);
    assert_eq!(panel(&mut w).fields.from, "ada");
    let p = panel(&mut w);
    p.fields.subject = "budget".into();
    p.fields.attachments = Some(true);
    p.scope = Scope::Everywhere;
    w.state_mut().execute("ui.advancedSearch", json!({"search": true})).unwrap();
    w.run_steps(2);
    // The keyword left alone keeps its spelling; the new ones come after it.
    assert_eq!(search(&w), "lunch FROM: ada subject:budget has:attachment");
    assert_eq!(w.state().session.view.scope, Scope::Everywhere);
    assert!(!is_open(&w));
    assert_eq!(w.state().search_text, search(&w));
    // Opened again, it shows what the box says.
    open(&mut w);
    let p = panel(&mut w);
    assert_eq!((p.fields.subject.as_str(), p.fields.attachments), ("budget", Some(true)));
}

#[test]
fn changing_only_where_it_looks_leaves_the_text() {
    let mut w = window();
    set_search(&mut w, "Categories:Red  from: ada");
    for scope in [Scope::Subfolders, Scope::Mailbox, Scope::Everywhere, Scope::Folder] {
        open(&mut w);
        panel(&mut w).scope = scope;
        w.state_mut().execute("ui.advancedSearch", json!({"search": true})).unwrap();
        w.run_steps(2);
        assert_eq!(search(&w), "Categories:Red  from: ada");
        assert_eq!(w.state().session.view.scope, scope);
    }
}

#[test]
fn search_and_enter_wait_for_the_days_to_be_dates() {
    let mut w = window();
    open(&mut w);
    panel(&mut w).received_from = "someday".into();
    w.run_steps(2);
    assert!(w.query_by_label(tl!("Not a date")).is_some(), "said in words");
    assert!(panel(&mut w).query().is_none());
    assert!(w.state_mut().execute("ui.advancedSearch", json!({"search": true})).is_err());
    assert!(is_open(&w), "still open, as it was");
    // The wrong way round.
    let p = panel(&mut w);
    (p.received_from, p.received_to) = ("2026-10-22".into(), "10/1/2026".into());
    w.run_steps(2);
    assert!(w.query_by_label(tl!("Ends before it starts")).is_some());
    assert!(panel(&mut w).query().is_none());
    panel(&mut w).received_to = "10/22/2026".into();
    w.state_mut().execute("ui.advancedSearch", json!({"search": true})).unwrap();
    assert_eq!(search(&w), "received:2026-10-22");
}

#[test]
fn escape_in_a_list_of_the_panel_closes_only_the_list() {
    let mut w = window();
    set_search(&mut w, "budget");
    open(&mut w);
    panel(&mut w).fields.subject = "q4".into();
    w.get_by_label(&format!("+ {}", tl!("Add filters"))).click();
    w.run_steps(2);
    assert!(egui::Popup::is_any_open(&w.ctx), "the menu is open");
    testing::key(&mut w, Key::Escape);
    assert!(!egui::Popup::is_any_open(&w.ctx), "the menu closed");
    assert_eq!(w.state().advanced_search.as_ref().map(|p| p.fields.subject.as_str()), Some("q4"));
    // The next closes the panel and leaves the search; the keyboard is back in the box.
    testing::key(&mut w, Key::Escape);
    assert!(!is_open(&w));
    assert_eq!(search(&w), "budget");
    w.run_steps(2);
    assert!(w.ctx.memory(|m| m.has_focus(crate::chrome::search_box_id())));
    // And the one after that is the box's: it clears the search.
    testing::key(&mut w, Key::Escape);
    assert_eq!(search(&w), "");
}

#[test]
fn a_held_escape_closes_the_panel_and_clears_nothing() {
    let mut w = window();
    set_search(&mut w, "budget");
    open(&mut w);
    testing::held(&mut w, Key::Escape);
    assert!(!is_open(&w));
    assert_eq!(search(&w), "budget");
}

#[test]
fn escape_with_the_keyboard_in_the_box_closes_the_panel_first() {
    let mut w = window();
    set_search(&mut w, "budget");
    w.state_mut().execute("ui.focusSearch", json!({})).unwrap();
    w.run_steps(3);
    open(&mut w);
    testing::key(&mut w, Key::Escape);
    assert!(!is_open(&w));
    assert_eq!(search(&w), "budget");
}

#[test]
fn enter_in_a_field_searches() {
    let mut w = window();
    open(&mut w);
    let r = w.get_by_role_and_label(egui::accesskit::Role::TextInput, tl!("Subject")).rect();
    testing::click(&mut w, r.center(), PointerButton::Primary, 1);
    testing::text(&mut w, "budget");
    testing::key(&mut w, Key::Enter);
    assert_eq!(search(&w), "subject:budget");
    assert!(!is_open(&w));
}

#[test]
fn a_click_outside_closes_the_panel() {
    let mut w = window();
    open(&mut w);
    let r = w.state().hit.rect("advancedSearch", 0).unwrap();
    testing::click(&mut w, egui::pos2(r.max.x + 40.0, r.max.y + 40.0), PointerButton::Primary, 1);
    assert!(!is_open(&w));
}

#[test]
fn add_filters_is_kept_and_what_the_box_holds_is_shown() {
    let mut w = window();
    set_search(&mut w, "cc:bob");
    open(&mut w);
    // Shown for the box, not ticked for good.
    assert!(w.query_by_role_and_label(egui::accesskit::Role::TextInput, tl!("Cc")).is_some());
    assert_eq!(w.state().ui.advanced_filters, Filters::default());
    // Unticked, it is emptied, and Search takes it out of the box.
    w.get_by_label(&format!("+ {}", tl!("Add filters"))).click();
    w.run_steps(2);
    w.get_by_role_and_label(egui::accesskit::Role::CheckBox, tl!("Cc")).click();
    w.run_steps(2);
    assert_eq!(panel(&mut w).fields.cc, "");
    if !egui::Popup::is_any_open(&w.ctx) {
        w.get_by_label(&format!("+ {}", tl!("Add filters"))).click();
        w.run_steps(2);
    }
    w.get_by_role_and_label(egui::accesskit::Role::CheckBox, tl!("Importance")).click();
    w.run_steps(2);
    assert_eq!(w.state().ui.advanced_filters, Filters { importance: true, ..Filters::default() });
    if egui::Popup::is_any_open(&w.ctx) {
        testing::key(&mut w, Key::Escape);
    }
    w.state_mut().execute("ui.advancedSearch", json!({"search": true})).unwrap();
    assert_eq!(search(&w), "");
    // The choice stays for the next opening, and in `ui.json`.
    open(&mut w);
    assert!(w.query_by_role_and_label(egui::accesskit::Role::ComboBox, tl!("Importance")).is_some());
    let saved = serde_json::to_value(&w.state().ui).unwrap();
    assert_eq!(saved["advancedFilters"], json!({"cc": false, "read": false, "importance": true}));
    let back: crate::UiState = serde_json::from_value(saved).unwrap();
    assert!(back.advanced_filters.importance);
    // Settings from before it: none ticked.
    let old: crate::UiState = serde_json::from_str("{}").unwrap();
    assert_eq!(old.advanced_filters, Filters::default());
}

#[test]
fn clear_filters_empties_the_fields_and_keeps_where_it_looks() {
    let mut w = window();
    set_search(&mut w, "lunch from:ada received:2026-10-01..2026-10-22");
    open(&mut w);
    panel(&mut w).scope = Scope::Mailbox;
    w.get_by_label(tl!("Clear filters")).click();
    w.run_steps(2);
    let p = panel(&mut w);
    assert!(p.fields.is_empty() && p.received_from.is_empty() && p.received_to.is_empty());
    assert_eq!(p.scope, Scope::Mailbox);
    w.state_mut().execute("ui.advancedSearch", json!({"search": true})).unwrap();
    assert_eq!(search(&w), "lunch");
}

#[test]
fn the_panel_and_the_suggestions_never_show_together() {
    let mut w = window();
    w.state_mut().execute("ui.focusSearch", json!({})).unwrap();
    w.run_steps(3);
    testing::text(&mut w, "cat");
    w.run_steps(3);
    assert!(crate::search_suggest::Suggesting::is_open(&w.ctx), "the list offers category:");
    click_at(&mut w, "advancedSearchButton");
    assert!(is_open(&w));
    w.run_steps(3);
    assert!(!crate::search_suggest::Suggesting::is_open(&w.ctx));
}

#[test]
fn the_panel_closes_when_what_it_was_opened_for_goes() {
    let mut w = window();
    // The search changed under it.
    open(&mut w);
    set_search(&mut w, "other");
    assert!(!is_open(&w));
    // Where it looks changed under it, the words the same.
    set_search(&mut w, "budget");
    open(&mut w);
    w.state_mut().execute("view.search", json!({"query": "budget", "scope": "everywhere"})).unwrap();
    w.run_steps(2);
    assert!(!is_open(&w));
    // The window became too narrow for the search box.
    open(&mut w);
    w.set_size(egui::vec2(420.0, 600.0));
    w.run_steps(3);
    assert!(!w.state().search_box_shown && !is_open(&w));
    w.set_size(egui::vec2(1280.0, 800.0));
    w.run_steps(3);
    // Mail left the front.
    open(&mut w);
    w.state_mut().execute("view.calendar", json!({})).unwrap();
    w.run_steps(2);
    assert!(!is_open(&w));
    w.state_mut().execute("view.mail", json!({})).unwrap();
    w.run_steps(2);
    // File.
    open(&mut w);
    w.state_mut().execute("ui.backstage", json!({"value": true})).unwrap();
    w.run_steps(2);
    assert!(!is_open(&w));
    w.state_mut().execute("ui.backstage", json!({"value": false})).unwrap();
    w.run_steps(2);
    // A dialog.
    open(&mut w);
    w.state_mut().dialog = Some(crate::dialogs::Dialog::Refused { text: "Not now".into() });
    w.run_steps(2);
    assert!(!is_open(&w));
}

#[test]
fn the_command_takes_true_or_false_and_says_what_is_wrong() {
    let mut w = window();
    let run = |w: &mut Window, p: serde_json::Value| w.state_mut().execute("ui.advancedSearch", p);
    for junk in [json!({"open": "yes"}), json!({"search": 1}), json!({"open": true, "search": true}), json!({"open": [true]})] {
        assert!(run(&mut w, junk.clone()).is_err(), "{junk}");
        assert!(!is_open(&w), "{junk}");
    }
    assert!(run(&mut w, json!({"search": true})).is_err(), "nothing to search with");
    assert_eq!(run(&mut w, json!({"search": false})).unwrap()["open"], json!(false));
    assert_eq!(run(&mut w, json!({"open": false})).unwrap()["open"], json!(false));
    assert_eq!(run(&mut w, json!({})).unwrap()["open"], json!(true), "left out: it opens");
    assert_eq!(run(&mut w, json!({})).unwrap()["open"], json!(false), "and closes");
    for junk in [json!([]), json!("x"), json!(1), json!({"open": null}), json!({"search": {}})] {
        assert!(run(&mut w, junk.clone()).is_err(), "{junk}");
        assert!(!is_open(&w), "{junk}");
    }
    // No parameters at all is as `{}`.
    assert_eq!(run(&mut w, json!(null)).unwrap()["open"], json!(true));
    assert!(run(&mut w, json!({"open": false})).is_ok());
    // Not with People in front, nor with a dialog over the window; closing always is.
    w.state_mut().execute("view.people", json!({})).unwrap();
    w.run_steps(2);
    assert!(run(&mut w, json!({"open": true})).is_err());
    assert!(run(&mut w, json!({"open": false})).is_ok());
    w.state_mut().execute("view.mail", json!({})).unwrap();
    w.state_mut().dialog = Some(crate::dialogs::Dialog::Refused { text: "Not now".into() });
    w.run_steps(2);
    assert!(run(&mut w, json!({"open": true})).is_err());
}

#[test]
fn the_button_is_mails_alone() {
    let mut w = window();
    for (module, shown) in [("view.calendar", false), ("view.tasks", false), ("view.people", false), ("view.mail", true)] {
        w.state_mut().execute(module, json!({})).unwrap();
        w.run_steps(3);
        assert_eq!(w.state().hit.rect("advancedSearchButton", 0).is_some(), shown, "{module}");
    }
    assert!(w.query_by_label(tl!("Advanced Search")).is_some(), "named for screen readers");
}

/// A small window, and a large interface: the panel stays on screen, its labels above the fields
/// when it is narrow.
#[test]
fn the_panel_fits_a_small_window() {
    for (size, percent) in [(egui::vec2(640.0, 480.0), 200.0), (egui::vec2(1280.0, 800.0), 80.0), (egui::vec2(560.0, 420.0), 100.0)] {
        let mut w = window();
        w.state_mut().execute("ui.interfaceSize", json!({"value": percent})).unwrap();
        w.set_size(size);
        w.run_steps(4);
        if !w.state().search_box_shown {
            assert!(w.state_mut().execute("ui.advancedSearch", json!({"open": true})).is_err(), "no box to open it under");
            continue;
        }
        open(&mut w);
        let r = w.state().hit.rect("advancedSearch", 0).unwrap();
        let screen = w.ctx.content_rect();
        assert!(r.min.x >= screen.min.x && r.max.x <= screen.max.x + 0.5, "{size:?} {percent}: {r:?} in {screen:?}");
        assert!(r.max.y <= screen.max.y + 0.5, "{size:?} {percent}: {r:?} in {screen:?}");
    }
}
