// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Calendar's Copy, Cut and Paste chosen in Command Search (owner decision D4; CodeRabbit on
//! #1665, dialogs.rs:1858): the search window still held the keyboard when it ran them, so each was
//! refused with "Close what is open first". They now run once the search window has gone, and the
//! guard still refuses them while a real dialog is over the Calendar.
//!
//! One module of `calendar_selection_tests`, so `super::*` is that file's fixtures (`calendar`,
//! `event`, `select`, `click`, `after_noon`, `date`, `at`, `Window`, `Id`, `DateTime`, `Modifiers`,
//! `PointerButton`), and `super::super` is `calendar_view`'s own `Clip` and `CLIP_ID`. The window
//! is the real one; the items are the sample's invented ones; every act is a command the owner
//! picks or a pointer event. No file, server, real account or wall clock.

use serde_json::json;

use super::super::{CLIP_ID, Clip};
use super::*;

fn clipboard(w: &Window) -> Option<Clip> {
    w.ctx.data(|data| data.get_temp::<Clip>(egui::Id::unique(CLIP_ID)))
}

/// Open Command Search on `id`, press Enter on the first match, and let the window settle.
fn search_and_run(w: &mut Window, id: &str) {
    w.state_mut().execute("ui.dialog", json!({"name": "commands", "query": id})).expect("Command Search opens");
    w.run_steps(3);
    assert_eq!(crate::people_view::testing::dialog(w), Some("commands"), "fixture: Command Search is open");
    for pressed in [true, false] {
        w.input_mut().events.push(egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
        w.step();
    }
    w.run_steps(8);
}

/// Whatever the window said last, as text: a refusal is said in the status bar or a message.
fn refused(w: &Window) -> bool {
    let status = w.state().status_msg.as_ref().map(|(said, _)| said.clone()).unwrap_or_default();
    status.contains("Close what is open first") || crate::people_view::testing::dialog(w) == Some("refused")
}

#[test]
fn calendar_copy_cut_and_paste_run_from_command_search() {
    let mut w = calendar();
    let lunch = event(&w, "Lunch with Dev");
    select(&mut w, &[(lunch.id, None)]);

    search_and_run(&mut w, "ui.calendarCopy");
    assert!(!refused(&w), "Copy from Command Search is not refused as if the search were still open");
    assert!(w.state().after_search.is_none(), "it ran");
    let copy = clipboard(&w).expect("Copy from Command Search puts the selected set on the calendar's clipboard");
    assert!(!copy.cut && copy.items.iter().map(|item| item.id).eq([lunch.id]), "a copy of Lunch");

    search_and_run(&mut w, "ui.calendarCut");
    assert!(!refused(&w), "Cut from Command Search is not refused");
    let cut = clipboard(&w).expect("Cut from Command Search puts the set on the clipboard");
    assert!(cut.cut && cut.items.iter().map(|item| item.id).eq([lunch.id]), "a cut of Lunch");

    let picked = after_noon(&w, 3.0);
    click(&mut w, picked, Modifiers::NONE, PointerButton::Primary);
    search_and_run(&mut w, "ui.calendarPaste");
    assert!(!refused(&w), "Paste from Command Search is not refused");
    assert_eq!(
        w.state().session.mailbox.event(lunch.id).map(|moved| moved.start),
        Some(at(date(10, 14), 15, 0)),
        "Paste from Command Search moved Lunch to the picked time"
    );
}

#[test]
fn a_real_dialog_over_the_calendar_still_refuses_copy_cut_and_paste() {
    let mut w = calendar();
    let lunch = event(&w, "Lunch with Dev");
    select(&mut w, &[(lunch.id, None)]);
    w.state_mut().execute("ui.dialog", json!({"name": "about"})).expect("the About window opens");
    w.run_steps(3);
    assert_eq!(crate::people_view::testing::dialog(&w), Some("about"), "fixture: a real dialog is open");
    for (id, said) in [
        ("ui.calendarCopy", "Close what is open first, then copy"),
        ("ui.calendarCut", "Close what is open first, then cut"),
        ("ui.calendarPaste", "Close what is open first, then paste"),
    ] {
        assert_eq!(w.state_mut().execute(id, json!({})), Err(said.to_string()), "{id} is refused while a dialog is open");
    }
    assert!(clipboard(&w).is_none(), "nothing was copied or cut");

    // A hand-over from Command Search that finds a real dialog over the Calendar when its wait is
    // up runs all the same, and is refused for it.
    w.state_mut().after_search = Some(("ui.calendarCopy".to_string(), 2));
    w.run_steps(6);
    assert!(w.state().after_search.is_none(), "the hand-over ran");
    assert!(clipboard(&w).is_none(), "and copied nothing while the dialog was open");
}
