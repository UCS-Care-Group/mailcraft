// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #308 in the window: the list under the search box, its keys, a click, and Escape.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use serde_json::json;

use super::Suggesting;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// The sample mailbox with the keyboard in the search box.
fn window() -> Harness<'static, MailApp> {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    app.execute("ui.focusSearch", json!({})).unwrap();
    let mut harness = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 900.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(4);
    harness
}

fn typed(h: &mut Harness<'static, MailApp>, text: &str) {
    h.event(egui::Event::Text(text.into()));
    h.run_steps(3);
}

fn key(h: &mut Harness<'static, MailApp>, key: egui::Key) {
    for pressed in [true, false] {
        h.event(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
}

/// What the list offers now.
fn offered(h: &Harness<'static, MailApp>) -> Vec<String> {
    Suggesting::load(&h.ctx).items.into_iter().map(|s| s.label).collect()
}

fn search(h: &Harness<'static, MailApp>) -> String {
    h.state().session.view.search.clone()
}

/// Whether the search box has the keyboard.
fn in_the_box(h: &Harness<'static, MailApp>) -> bool {
    h.ctx.memory(|m| m.has_focus(crate::chrome::search_box_id()))
}

/// The start of a keyword offers it; Tab takes it and the mailbox's categories follow; the arrow
/// keys and Enter take one, which is searched for at once.
#[test]
fn a_keyword_then_a_category_from_the_keyboard() {
    let mut h = window();
    typed(&mut h, "cat");
    assert_eq!(offered(&h), ["category:"]);
    assert!(h.query_by_label("category:").is_some(), "the list is drawn");
    key(&mut h, egui::Key::Tab);
    assert_eq!(search(&h), "category:");
    let names: Vec<String> = h.state().session.mailbox.categories.iter().map(|c| c.name.clone()).collect();
    assert_eq!(offered(&h), names.iter().take(8).cloned().collect::<Vec<_>>());
    typed(&mut h, "gr");
    assert_eq!(offered(&h), ["Green category"]);
    key(&mut h, egui::Key::ArrowDown);
    key(&mut h, egui::Key::Enter);
    assert_eq!(search(&h), "category:\"Green category\" ");
    assert!(offered(&h).is_empty(), "nothing for the next word yet");
    let rows = h.state_mut().session.rows();
    assert!(!rows.is_empty());
    // The keyboard is still in the box: typing goes on after it.
    typed(&mut h, "is:u");
    assert_eq!(search(&h), "category:\"Green category\" is:u");
    assert_eq!(offered(&h), ["unread"]);
}

/// Until an arrow key chooses one, Enter takes nothing and the search is as typed.
#[test]
fn enter_takes_only_what_was_chosen() {
    let mut h = window();
    typed(&mut h, "to");
    assert_eq!(offered(&h), ["to:"]);
    key(&mut h, egui::Key::Enter);
    assert_eq!(search(&h), "to");
    assert!(offered(&h).is_empty(), "the box gave up the keyboard, as before, and the list went");
}

/// Up from nothing chosen goes to the last, and round.
#[test]
fn up_goes_round_to_the_last() {
    let mut h = window();
    typed(&mut h, "is:");
    key(&mut h, egui::Key::ArrowUp);
    key(&mut h, egui::Key::Enter);
    assert_eq!(search(&h), "is:important ");
}

/// With Shift held, Tab is not the list's: nothing is taken, and the keyboard leaves the box, as
/// it does with no list. The list closes.
#[test]
fn shift_tab_takes_nothing() {
    let mut h = window();
    typed(&mut h, "cat");
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Tab);
    h.run_steps(3);
    assert_eq!(search(&h), "cat");
    assert!(!in_the_box(&h), "the keyboard left the box");
    assert!(h.query_by_label("category:").is_none(), "the list closed");
}

/// With no list, Tab leaves the box as it did: what is typed next doesn't go in.
#[test]
fn tab_with_no_list_leaves_the_box() {
    let mut h = window();
    typed(&mut h, "budget");
    assert!(offered(&h).is_empty());
    key(&mut h, egui::Key::Tab);
    typed(&mut h, "x");
    assert_eq!(search(&h), "budget");
}

/// A key that comes in the same frame as typing takes nothing offered for the text before it:
/// `c` then `a` and Tab at once is `ca`, never `cc:` with the `a` after it.
#[test]
fn a_take_is_only_of_what_was_offered_for_the_text_now() {
    let mut h = window();
    typed(&mut h, "c");
    assert_eq!(offered(&h), ["cc:", "category:"]);
    h.input_mut().events.extend([
        egui::Event::Text("a".into()),
        egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE },
        egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::NONE },
    ]);
    h.step();
    h.run_steps(3);
    assert_eq!(search(&h), "ca");
    // The list offered for `ca` takes Tab from the very next frame.
    assert_eq!(offered(&h), ["category:"]);
    h.input_mut().events.extend([
        egui::Event::Text("t".into()),
        egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE },
        egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::NONE },
    ]);
    h.step();
    h.run_steps(3);
    key(&mut h, egui::Key::Tab);
    assert_eq!(search(&h), "category:");
}

/// While a message is being written, Escape in the search box closes the list, then clears the
/// search, and the message stays open (#365).
#[test]
fn escape_closes_the_list_and_not_the_message_being_written() {
    let mut h = window();
    h.state_mut().execute("mail.new", json!({"subject": "Half written"})).unwrap();
    h.state_mut().search_focus = true;
    h.run_steps(3);
    typed(&mut h, "has:");
    assert_eq!(offered(&h), ["attachment"]);
    key(&mut h, egui::Key::Escape);
    assert!(h.state().session.compose.is_some(), "the message is still open");
    assert_eq!(search(&h), "has:");
    assert!(h.query_by_label("attachment").is_none());
    key(&mut h, egui::Key::Escape);
    assert_eq!(search(&h), "");
    assert!(h.state().session.compose.is_some(), "still open");
}

/// Escape with a dialog over the window is the dialog's: it closes the dialog, and the search, and
/// the list under the box, stay as they were.
#[test]
fn escape_for_a_dialog_leaves_the_search() {
    for text in ["budget", "cat"] {
        let mut h = window();
        typed(&mut h, text);
        h.state_mut().dialog = Some(crate::dialogs::Dialog::Refused { text: "Not now".into() });
        h.run_steps(3);
        key(&mut h, egui::Key::Escape);
        assert!(h.state().dialog.is_none(), "{text}");
        assert_eq!(search(&h), text);
    }
}

/// A held Escape that closed a dialog over the window leaves the list under the box open, as it
/// leaves the search: its repeats are not a press of their own (#393).
#[test]
fn a_held_escape_for_a_dialog_leaves_the_list() {
    let mut h = window();
    typed(&mut h, "cat");
    assert!(h.query_by_label("category:").is_some());
    h.state_mut().dialog = Some(crate::dialogs::Dialog::Refused { text: "Not now".into() });
    h.run_steps(3);
    crate::people_view::testing::held(&mut h, egui::Key::Escape);
    assert!(h.state().dialog.is_none());
    assert_eq!(search(&h), "cat");
    assert!(h.query_by_label("category:").is_some(), "the list is still open");
}

/// Every row of the list has a name a screen reader says, and the list adds no control without
/// one (egui 0.37's check, which the window's harness leaves off).
#[test]
fn the_list_adds_nothing_without_a_name() {
    let mut h = window();
    let before = h.unnamed_widgets();
    typed(&mut h, "category:");
    assert!(h.query_by_label("Green category").is_some());
    let added: Vec<String> = h.unnamed_widgets().into_iter().filter(|w| !before.contains(w)).collect();
    assert_eq!(added, Vec::<String>::new());
}

/// A click on a suggestion takes it.
#[test]
fn a_click_takes_one() {
    let mut h = window();
    typed(&mut h, "is:");
    h.get_by_label("flagged").click();
    h.run_steps(3);
    assert_eq!(search(&h), "is:flagged ");
    assert!(!h.state_mut().session.rows().is_empty());
}

/// Escape closes the list and leaves the search; typing opens it again; Escape with the list
/// closed clears the search, as before.
#[test]
fn escape_closes_the_list_then_clears() {
    let mut h = window();
    typed(&mut h, "has:");
    assert!(h.query_by_label("attachment").is_some());
    key(&mut h, egui::Key::Escape);
    assert_eq!(search(&h), "has:");
    assert!(h.query_by_label("attachment").is_none(), "closed");
    key(&mut h, egui::Key::Backspace);
    typed(&mut h, ":");
    assert!(h.query_by_label("attachment").is_some(), "open again once the text changes");
    key(&mut h, egui::Key::Escape);
    key(&mut h, egui::Key::Escape);
    assert_eq!(search(&h), "");
}

/// People has no keywords: nothing is offered there.
#[test]
fn people_search_offers_nothing() {
    let mut h = window();
    h.state_mut().execute("view.people", json!({})).unwrap();
    h.state_mut().search_focus = true;
    h.run_steps(3);
    typed(&mut h, "cat");
    assert!(h.query_by_label("category:").is_none());
}

/// A dialog is open while the list shows. The list is not drawn over the dialog. When the dialog
/// closes, the list is back.
#[test]
fn the_list_is_not_drawn_over_a_dialog() {
    let mut h = window();
    typed(&mut h, "cat");
    assert!(h.query_by_label("category:").is_some());
    h.state_mut().dialog = Some(crate::dialogs::Dialog::Refused { text: "Not now".into() });
    h.run_steps(3);
    assert!(h.query_by_label("category:").is_none(), "not over the dialog");
    key(&mut h, egui::Key::Escape);
    assert!(h.state().dialog.is_none());
    assert!(h.query_by_label("category:").is_some(), "back once the dialog is closed");
}

/// A category is renamed while the list shows it. The list then shows the new name.
#[test]
fn a_renamed_category_is_offered_by_its_new_name() {
    let mut h = window();
    typed(&mut h, "category:gr");
    assert_eq!(offered(&h), ["Green category"]);
    h.state_mut().execute("category.rename", json!({"from": "Green category", "to": "Grass"})).unwrap();
    h.run_steps(3);
    assert_eq!(offered(&h), ["Grass"]);
}

/// Escape closes the list. Moving the cursor to another word opens it again, for that word.
#[test]
fn escape_closes_the_list_until_the_cursor_moves() {
    let mut h = window();
    typed(&mut h, "cat is:");
    assert!(h.query_by_label("unread").is_some());
    key(&mut h, egui::Key::Escape);
    assert!(h.query_by_label("unread").is_none(), "closed");
    for _ in 0..4 {
        key(&mut h, egui::Key::ArrowLeft);
    }
    assert_eq!(offered(&h), ["category:"]);
    assert!(h.query_by_label("category:").is_some(), "open again, for `cat`");
    assert_eq!(search(&h), "cat is:");
}

/// Escape closes the list. After leaving the box and coming back, the list shows again.
#[test]
fn escape_is_forgotten_when_the_box_is_left() {
    let mut h = window();
    typed(&mut h, "is:");
    key(&mut h, egui::Key::Escape);
    assert!(h.query_by_label("unread").is_none(), "closed");
    h.ctx.memory_mut(|m| m.surrender_focus(crate::chrome::search_box_id()));
    h.run_steps(3);
    assert!(!in_the_box(&h));
    h.state_mut().search_focus = true;
    h.run_steps(3);
    assert!(h.query_by_label("unread").is_some(), "open again");
    assert_eq!(search(&h), "is:");
}

/// The pointer is on the list and Enter is pressed with nothing chosen. The box gives up the
/// keyboard, as it did before the list, and the list closes with it.
#[test]
fn enter_with_the_pointer_on_the_list_closes_it() {
    let mut h = window();
    typed(&mut h, "is:");
    h.get_by_label("flagged").hover();
    h.run_steps(3);
    key(&mut h, egui::Key::Enter);
    assert!(!in_the_box(&h), "Enter left the box");
    assert!(h.query_by_label("flagged").is_none(), "the list closed");
    assert_eq!(search(&h), "is:");
}

/// A held key is one press on the list too (#391, #393): Escape closes the list and leaves the
/// search and the message being written; Tab or Enter takes one suggestion and the keyboard stays
/// in the box.
#[test]
fn a_held_key_is_one_press_on_the_list() {
    use crate::people_view::testing::held;
    let mut h = window();
    h.state_mut().execute("mail.new", json!({"subject": "Half written"})).unwrap();
    h.state_mut().search_focus = true;
    h.run_steps(3);
    typed(&mut h, "has:");
    assert_eq!(offered(&h), ["attachment"]);
    held(&mut h, egui::Key::Escape);
    assert!(h.query_by_label("attachment").is_none(), "the list closed");
    assert_eq!(search(&h), "has:");
    assert!(h.state().session.compose.is_some(), "the message is still open");

    for (how, keys) in [("Tab", &[][..]), ("Enter", &[egui::Key::ArrowDown][..])] {
        let mut h = window();
        typed(&mut h, "cat");
        for k in keys {
            key(&mut h, *k);
        }
        held(&mut h, if how == "Tab" { egui::Key::Tab } else { egui::Key::Enter });
        assert_eq!(search(&h), "category:", "{how}: one suggestion taken");
        assert!(in_the_box(&h), "{how}: the keyboard is still in the box");
        assert!(!offered(&h).is_empty(), "{how}: the categories are offered");
    }
}
