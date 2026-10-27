// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Escape in the search box while a message is being written (#365): the box's, not the
//! message's; and a dialog's when one is over the window. In People the box shows People's own
//! search, and everywhere else the mail's (#366).

use egui::{Key, PointerButton};
use serde_json::json;

use crate::people_view::testing::{self, Window};

/// The sample mailbox with a message being written, and the keyboard in the search box.
fn window() -> Window {
    let mut h = testing::window("view.mail");
    let app = h.state_mut();
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    app.execute("mail.new", json!({"subject": "Half written"})).unwrap();
    app.execute("ui.focusSearch", json!({})).unwrap();
    h.run_steps(4);
    h
}

fn search(h: &Window) -> String {
    h.state().session.view.search.clone()
}

fn writing(h: &Window) -> bool {
    h.state().session.compose.as_ref().is_some_and(|c| c.subject == "Half written")
}

fn refused(h: &mut Window) {
    h.state_mut().dialog = Some(crate::dialogs::Dialog::Refused { text: "Not now".into() });
    h.run_steps(3);
}

/// Escape in the search box clears the search and leaves the message being written open; the
/// box has given up the keyboard, so the next Escape is the message's again and closes it.
#[test]
fn escape_in_the_search_box_leaves_the_message_open() {
    let mut h = window();
    testing::text(&mut h, "budget");
    assert_eq!(search(&h), "budget");
    testing::key(&mut h, Key::Escape);
    assert_eq!(search(&h), "");
    assert!(writing(&h), "the message is still open");
    testing::key(&mut h, Key::Escape);
    assert!(h.state().session.compose.is_none());
}

/// With a dialog over the window, Escape is the dialog's: it closes the dialog, and the search
/// and the message stay as they were.
#[test]
fn escape_for_a_dialog_leaves_the_search() {
    let mut h = window();
    testing::text(&mut h, "budget");
    refused(&mut h);
    testing::key(&mut h, Key::Escape);
    assert!(h.state().dialog.is_none());
    assert_eq!(search(&h), "budget");
    assert!(writing(&h));
}

/// A held Escape is one press. Over a dialog, its repeats leave the search in the box; in the
/// box, they don't close the message once the box has cleared the search; nor over a dialog when
/// the box doesn't have the keyboard.
#[test]
fn a_held_escape_is_one_press() {
    let mut h = window();
    testing::text(&mut h, "budget");
    refused(&mut h);
    testing::held(&mut h, Key::Escape);
    assert!(h.state().dialog.is_none());
    assert_eq!(search(&h), "budget", "the dialog had it");
    assert!(writing(&h));
    testing::held(&mut h, Key::Escape);
    assert_eq!(search(&h), "");
    assert!(writing(&h), "the search box had it");
    refused(&mut h);
    testing::held(&mut h, Key::Escape);
    assert!(h.state().dialog.is_none());
    assert!(writing(&h), "the dialog had it");
}

fn people_search(h: &Window) -> String {
    h.state().session.view.people_search.clone()
}

fn in_the_box(h: &Window) -> String {
    h.state().search_text.clone()
}

/// In People, letters typed in the box and a click on a contact they found: the box keeps the
/// letters, and the list stays as they filter it (#366).
#[test]
fn people_keeps_its_search_in_the_box_after_a_click_outside() {
    let mut h = testing::window("view.people");
    let _ = h.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    h.run_steps(2);
    click_in_the_box(&mut h);
    testing::text(&mut h, "travel");
    assert_eq!(people_search(&h), "travel");
    let found = h.state_mut().execute("people.list", json!({})).unwrap();
    let id = found["contacts"][0]["id"].as_u64().unwrap();
    let row = h.state().hit.rect("contact", id).unwrap();
    testing::click(&mut h, row.center(), PointerButton::Primary, 1);
    h.run_steps(3);
    assert_eq!(h.state().session.sel.contact, Some(id), "the click was on the contact");
    assert!(!has_the_keyboard(&h));
    assert_eq!(in_the_box(&h), "travel", "the box keeps its letters");
    assert_eq!(people_search(&h), "travel", "the list stays filtered");
    assert_eq!(search(&h), "", "the mail isn't searched");
}

/// The box shows the search of what is in front: People's in People, the mail's in Mail, each
/// changed by a command, and each back when its place is.
#[test]
fn the_box_follows_the_search_of_what_is_in_front() {
    let mut h = testing::window("view.mail");
    let _ = h.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    let run = |h: &mut Window, id: &str, p: serde_json::Value| {
        h.state_mut().execute(id, p).unwrap();
        h.run_steps(3);
    };
    run(&mut h, "view.search", json!({"query": "budget"}));
    assert_eq!(in_the_box(&h), "budget");
    run(&mut h, "view.people", json!({}));
    assert_eq!(in_the_box(&h), "", "People isn't searched");
    run(&mut h, "people.search", json!({"query": "travel"}));
    assert_eq!(in_the_box(&h), "travel");
    run(&mut h, "view.mail", json!({}));
    assert_eq!(in_the_box(&h), "budget");
    run(&mut h, "view.people", json!({}));
    assert_eq!(in_the_box(&h), "travel");
    run(&mut h, "people.search", json!({"query": ""}));
    assert_eq!(in_the_box(&h), "");
    assert_eq!(search(&h), "budget", "the mail's search stays");
}

/// Ctrl and a key, with the keyboard where it is.
fn chord(h: &mut Window, key: Key) {
    for pressed in [true, false] {
        h.input_mut().events.push(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::COMMAND });
    }
    h.run_steps(3);
}

/// A click in the box: in People the only way in, as Ctrl+E goes to Mail.
fn click_in_the_box(h: &mut Window) {
    let r = h.state().hit.rect("searchBox", 0).unwrap();
    testing::click(h, r.center(), PointerButton::Primary, 1);
}

fn has_the_keyboard(h: &Window) -> bool {
    h.ctx.memory(|m| m.has_focus(crate::chrome::search_box_id()))
}

/// The mail searched for "budget", and "tra" typed in People's box.
fn typed_in_people() -> Window {
    let mut h = testing::window("view.mail");
    let _ = h.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    h.state_mut().execute("view.search", json!({"query": "budget"})).unwrap();
    h.state_mut().execute("view.people", json!({})).unwrap();
    h.run_steps(3);
    click_in_the_box(&mut h);
    testing::text(&mut h, "tra");
    assert_eq!(people_search(&h), "tra", "the box showed People's search");
    h
}

/// Ctrl+1 and Ctrl+3 with the keyboard in the box: the box shows the search of what comes in
/// front at once, and what is typed next goes on at its end, into that search.
#[test]
fn the_box_swaps_searches_with_what_comes_in_front_while_typing() {
    let mut h = typed_in_people();
    chord(&mut h, Key::Num1);
    assert_eq!(h.state().session.view.module, mailcraft_engine::Module::Mail);
    assert_eq!(in_the_box(&h), "budget", "Mail's search");
    testing::text(&mut h, "s");
    assert_eq!(search(&h), "budgets");
    assert_eq!(people_search(&h), "tra");
    chord(&mut h, Key::Num3);
    assert_eq!(in_the_box(&h), "tra", "People's search");
    testing::text(&mut h, "s");
    assert_eq!(people_search(&h), "tras");
    assert_eq!(search(&h), "budgets");
}

/// Undo in the box after the other search came in front brings nothing of the one before back:
/// with the keyboard in the box, or after a click elsewhere and back in the box.
#[test]
fn undo_after_a_swap_keeps_to_the_search_in_front() {
    for clicked_away in [false, true] {
        let mut h = typed_in_people();
        // egui keeps a step to undo once the text has stood still for a second.
        h.run_steps(90);
        if clicked_away {
            let found = h.state_mut().execute("people.list", json!({})).unwrap();
            let id = found["contacts"][0]["id"].as_u64().unwrap();
            let row = h.state().hit.rect("contact", id).unwrap();
            testing::click(&mut h, row.center(), PointerButton::Primary, 1);
            assert!(!has_the_keyboard(&h));
            h.state_mut().execute("view.mail", json!({})).unwrap();
            h.run_steps(3);
            click_in_the_box(&mut h);
        } else {
            chord(&mut h, Key::Num1);
        }
        assert_eq!(in_the_box(&h), "budget", "clicked away: {clicked_away}");
        chord(&mut h, Key::Z);
        assert_eq!(in_the_box(&h), "budget", "clicked away: {clicked_away}");
        assert_eq!(search(&h), "budget", "clicked away: {clicked_away}");
    }
}

/// Ctrl+E after a search changed while the box was left (an agent's): the caret is at the end
/// of the search it shows, so what is typed goes on after it.
#[test]
fn the_caret_is_at_the_end_when_the_box_takes_the_keyboard() {
    let mut h = testing::window("view.mail");
    let _ = h.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    h.run_steps(2);
    click_in_the_box(&mut h);
    testing::text(&mut h, "tra");
    let folder = h.state().session.view.folder;
    let row = h.state().hit.rect("folder", folder).unwrap();
    testing::click(&mut h, row.center(), PointerButton::Primary, 1);
    assert!(!has_the_keyboard(&h));
    h.state_mut().execute("view.search", json!({"query": "budget"})).unwrap();
    h.run_steps(3);
    assert_eq!(in_the_box(&h), "budget");
    h.state_mut().execute("ui.focusSearch", json!({})).unwrap();
    h.run_steps(3);
    testing::text(&mut h, "s");
    assert_eq!(search(&h), "budgets");
}

/// The same letters in both searches, the caret back in them: after Ctrl+1 it is at the end all
/// the same.
#[test]
fn a_swap_puts_the_caret_at_the_end_with_the_same_letters_too() {
    let mut h = testing::window("view.mail");
    let _ = h.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    h.state_mut().execute("view.search", json!({"query": "tra"})).unwrap();
    h.state_mut().execute("view.people", json!({})).unwrap();
    h.run_steps(3);
    click_in_the_box(&mut h);
    testing::text(&mut h, "tra");
    testing::key(&mut h, Key::ArrowLeft);
    testing::key(&mut h, Key::ArrowLeft);
    chord(&mut h, Key::Num1);
    testing::text(&mut h, "s");
    assert_eq!(search(&h), "tras");
}
