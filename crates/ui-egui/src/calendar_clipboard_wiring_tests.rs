// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Calendar's Clipboard in the ribbon and in command search: a Clipboard group of the
//! Calendar's Home tab bound to the three window commands that already exist — `ui.calendarCopy`,
//! `ui.calendarCut` and `ui.calendarPaste` — and those three, and not the engine's own
//! `calendar.copy` or `calendar.paste`, offered by command search.
//!
use mailcraft_engine::Session;
use serde_json::json;

use crate::dialogs::{all_commands, score};
use crate::people_view::testing::{Window, click, now, window};
use crate::ribbon::{describe_tab, usable};
use crate::{MailApp, Services};

/// A window-less app on the sample mailbox, as of its today — nothing drawn, for what only reads
/// the description of a tab.
fn app(module: &str) -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(now(), 0), now(), 0), Services::default());
    app.autosave = false;
    app.execute(module, json!({})).expect("the module opens");
    app
}

/// The window a click can be made in: the sample's today (Wednesday 14 October 2026), the
/// reminders that would lie over the grid put away, and the Calendar's week in front.
fn calendar() -> Window {
    let mut w = window("view.calendar");
    w.state_mut().session.driver.set_auto_sync(false);
    let _ = w.state_mut().execute("ui.dismissReminder", json!({ "all": true }));
    let _ = w.state_mut().execute("view.week", json!({}));
    w.run_steps(3);
    w
}

/// The control of the Calendar's Home ribbon that runs `command`.
fn control(w: &Window, command: &str) -> crate::ribbon::Item {
    let described = describe_tab(w.state(), "Home");
    described
        .items()
        .find(|item| item.command.as_ref().is_some_and(|(id, _)| *id == command))
        .cloned()
        .unwrap_or_else(|| panic!("the Calendar's Home ribbon has no control for {command}"))
}

/// The sample's event whose title begins with `title`.
fn event(w: &Window, title: &str) -> mailcraft_model::Id {
    w.state()
        .session
        .mailbox
        .events
        .values()
        .find(|event| event.title.starts_with(title))
        .map(|event| event.id)
        .unwrap_or_else(|| panic!("the sample has no event \"{title}\""))
}

/// How many events of exactly this title the mailbox holds: a copy makes it two.
fn copies(w: &Window, title: &str) -> usize {
    w.state().session.mailbox.events.values().filter(|event| event.title == title).count()
}

/// A whole-day cell of the week in view, as the window's hit map records it: a click in it picks
/// that day, which is where a Paste puts what the clipboard holds.
fn all_day_cell(w: &Window) -> egui::Rect {
    for day in 12..=18u8 {
        let day = mailcraft_time::Date::new(2026, 10, day).expect("a day of the sample's week");
        let id = u64::try_from(day.to_days()).expect("a day the hit map can name");
        if let Some(cell) = w.state().hit.rect("allDay", id) {
            return cell;
        }
    }
    panic!("the week in view draws no day of its all-day row");
}

/// The Calendar's Home tab has a Clipboard group of its own, and it is the three existing window
/// commands, each run with no parameters and drawn with an icon the repository
/// already has.
#[test]
fn the_calendar_home_ribbon_has_a_clipboard_group_of_the_three_calendar_commands() {
    let a = app("view.calendar");
    let described = describe_tab(&a, "Home");
    let clipboard = described.groups.iter().find(|group| group.title == "Clipboard").unwrap_or_else(|| {
        let titles: Vec<&str> = described.groups.iter().map(|group| group.title).collect();
        panic!("the Calendar's Home tab has no Clipboard group; its groups are {titles:?}")
    });

    // Exactly the three: every control runs one of them; their presentation order is not prescribed.
    let mut commands: Vec<&str> = clipboard.items.iter().filter_map(|item| item.command.as_ref().map(|(id, _)| *id)).collect();
    commands.sort_unstable();
    assert_eq!(clipboard.items.len(), commands.len(), "every control of the Clipboard group runs a command");
    assert_eq!(
        commands,
        ["ui.calendarCopy", "ui.calendarCut", "ui.calendarPaste"],
        "the Clipboard group offers exactly the Calendar's Copy, Cut and Paste"
    );

    for item in &clipboard.items {
        let command = item.command.as_ref().unwrap_or_else(|| panic!("{} of the Clipboard group runs no command", item.key));
        let (id, params) = (command.0, &command.1);
        // The commands refuse any parameter: the control must not hand them one.
        assert_eq!(*params, json!({}), "{id} is given no parameters");
        assert!(!item.pending, "{id} is built, not one of classic Outlook's controls still to come");
        // No new drawing and no new asset: an icon the repository already paints.
        assert!(crate::icons::NAMES.contains(&item.icon), "{id} draws the existing icon \"{}\"", item.icon);
        assert!(!item.label.is_empty(), "{id} has a label");
    }
}

/// Paste is greyed until the Calendar's clipboard holds something *and* a time is picked in the
/// grid, and then it is not greyed even though nothing is selected and no appointment is in focus:
/// what enables it is the clip and the pick, never the focus.
#[test]
fn the_clipboard_paste_waits_for_a_clip_and_a_picked_time_not_for_a_focus() {
    let mut w = calendar();

    // Nothing selected and nothing copied: all three are greyed.
    assert!(!usable(w.state(), &control(&w, "ui.calendarCopy")), "Copy is greyed with nothing selected");
    assert!(!usable(w.state(), &control(&w, "ui.calendarCut")), "Cut is greyed with nothing selected");
    assert!(!usable(w.state(), &control(&w, "ui.calendarPaste")), "Paste is greyed with an empty clipboard");

    // One appointment of the sample's own selected: Copy and Cut can take it, Paste still has
    // nothing of its own to put.
    let lunch = event(&w, "Lunch with Dev");
    w.state_mut().execute("calendar.select", json!({ "id": lunch })).expect("the sample's appointment is selected");
    w.run_steps(2);
    assert!(usable(w.state(), &control(&w, "ui.calendarCopy")), "Copy takes the selected set");
    assert!(usable(w.state(), &control(&w, "ui.calendarCut")), "Cut takes the selected set");
    assert!(!usable(w.state(), &control(&w, "ui.calendarPaste")), "Paste has nothing of its own to put yet");

    // Copy it, as the ribbon's Copy does, and let go of the selection: the clipboard keeps the
    // copy, and Paste stays greyed because no time is picked.
    let answer = w.state_mut().execute("ui.calendarCopy", json!({})).expect("the Calendar's Copy");
    assert_eq!(answer, json!({ "items": 1, "cut": false }), "Copy answers only how many, and that it was a copy");
    w.state_mut().execute("calendar.select", json!({ "id": null })).expect("nothing is selected");
    w.run_steps(2);
    assert!(w.state().session.sel.event.is_none() && w.state().session.sel.events.is_empty(), "nothing is selected");
    assert!(!usable(w.state(), &control(&w, "ui.calendarPaste")), "Paste wants a time picked as well as a clipboard");

    // A click in the all-day row picks a day: from now on Paste can put the clipboard there,
    // though nothing is selected and no appointment is in focus.
    let cell = all_day_cell(&w);
    click(&mut w, egui::pos2(cell.center().x, cell.max.y - 3.0), egui::PointerButton::Primary, 1);
    assert!(w.state().session.sel.event.is_none() && w.state().session.sel.events.is_empty(), "a pick selects nothing");
    assert!(usable(w.state(), &control(&w, "ui.calendarPaste")), "Paste can put the clipboard at the picked day");

    // And it does, through the command the control runs.
    let pasted = w.state_mut().execute("ui.calendarPaste", json!({})).expect("Paste at the picked day");
    assert_eq!(pasted, json!({ "items": 1, "cut": false }), "Paste answers only how many, and that it was a copy");
    assert_eq!(copies(&w, "Lunch with Dev"), 2, "the copy came");
    assert!(w.state().session.mailbox.event(lunch).is_some(), "a copy leaves its source where it was");
}

/// Command search lists the Calendar's three and not the engine's raw paste, which has to be given
/// `items` and a `start` — and those three are greyed, not missing, while nothing is selected.
#[test]
fn command_search_offers_the_three_calendar_clipboard_commands_and_not_the_engines_paste() {
    let w = calendar();
    let app = w.state();
    // The paste excluded below is a real command, refused here on purpose: the caller would have to
    // name the items and the start (`crates/engine/src/cmd/calendar/selection.rs:31`), which nobody
    // types into a search box.
    assert!(app.session.registry.get("calendar.paste").is_some(), "fixture: the engine's paste exists");
    assert!(app.session.registry.get("calendar.copy").is_some(), "fixture: the engine's copy exists");

    let all = all_commands(app);
    let ids: Vec<&str> = all.iter().map(|command| command.0.as_str()).collect();
    assert!(!ids.contains(&"calendar.copy"), "the raw engine copy also requires explicit parameters, not a search action");
    for id in ["ui.calendarCopy", "ui.calendarCut", "ui.calendarPaste"] {
        assert!(ids.contains(&id), "command search lists {id}: {ids:?}");
    }
    assert!(!ids.contains(&"calendar.paste"), "the engine's calendar.paste, which must be given its items and its start, is not offered");

    // Each is found by the word a person types: the raw paste is the one a search for "paste" would
    // otherwise lead to.
    for (query, id) in [("copy", "ui.calendarCopy"), ("cut", "ui.calendarCut"), ("paste", "ui.calendarPaste")] {
        let entry = all.iter().find(|command| command.0 == id).unwrap_or_else(|| panic!("no {id} in command search"));
        assert!(score(query, &entry.1, &entry.2, &entry.0).is_some(), "{id} is found by \"{query}\"");
    }

    // Greyed with nothing selected and nothing copied, but there to be seen and to say why.
    for id in ["ui.calendarCopy", "ui.calendarCut", "ui.calendarPaste"] {
        let entry = all.iter().find(|command| command.0 == id).unwrap_or_else(|| panic!("no {id} in command search"));
        assert!(!entry.4, "{id} is greyed while there is nothing to copy or paste");
    }
}
