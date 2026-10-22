// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Keyboard shortcuts: the registry's `shortcut` fields, run when no text field has the keyboard.

use egui::{Key, Modifiers};
use mailcraft_engine::Module;
use serde_json::json;

use crate::MailApp;

/// The registry's name for a key.
pub fn key_name(k: Key) -> Option<&'static str> {
    Some(match k {
        Key::ArrowLeft => "ArrowLeft",
        Key::ArrowRight => "ArrowRight",
        Key::ArrowUp => "ArrowUp",
        Key::ArrowDown => "ArrowDown",
        Key::Home => "Home",
        Key::End => "End",
        Key::PageUp => "PageUp",
        Key::PageDown => "PageDown",
        Key::Enter => "Enter",
        Key::Tab => "Tab",
        Key::Backspace => "Backspace",
        Key::Delete => "Delete",
        Key::Escape => "Escape",
        Key::Space => "Space",
        Key::Insert => "Insert",
        Key::Num0 => "0",
        Key::Num1 => "1",
        Key::Num2 => "2",
        Key::Num3 => "3",
        Key::Num4 => "4",
        Key::Num5 => "5",
        Key::Num6 => "6",
        Key::Num7 => "7",
        Key::Num8 => "8",
        Key::Num9 => "9",
        Key::F1 => "F1",
        Key::F2 => "F2",
        Key::F3 => "F3",
        Key::F4 => "F4",
        Key::F5 => "F5",
        Key::F6 => "F6",
        Key::F7 => "F7",
        Key::F8 => "F8",
        Key::F9 => "F9",
        Key::F10 => "F10",
        Key::F11 => "F11",
        Key::F12 => "F12",
        Key::A => "A",
        Key::B => "B",
        Key::C => "C",
        Key::D => "D",
        Key::E => "E",
        Key::F => "F",
        Key::G => "G",
        Key::H => "H",
        Key::I => "I",
        Key::J => "J",
        Key::K => "K",
        Key::L => "L",
        Key::M => "M",
        Key::N => "N",
        Key::O => "O",
        Key::P => "P",
        Key::Q => "Q",
        Key::R => "R",
        Key::S => "S",
        Key::T => "T",
        Key::U => "U",
        Key::V => "V",
        Key::W => "W",
        Key::X => "X",
        Key::Y => "Y",
        Key::Z => "Z",
        _ => return None,
    })
}

/// A key with its modifiers as the registry writes shortcuts (`Mod+Shift+K`).
pub fn combo(m: Modifiers, key: &str) -> String {
    let mut s = String::new();
    if m.command {
        s.push_str("Mod+");
    }
    if m.mac_cmd && m.ctrl {
        s.push_str("Ctrl+");
    }
    if m.alt {
        s.push_str("Alt+");
    }
    if m.shift {
        s.push_str("Shift+");
    }
    s.push_str(key);
    s
}

/// The module a command belongs to, when it only makes sense there (Delete deletes a message in
/// Mail, a contact in People, a task in Tasks).
fn module_of(id: &str) -> Option<Module> {
    match id.split('.').next()? {
        "mail" | "folder" | "compose" => Some(Module::Mail),
        "calendar" => Some(Module::Calendar),
        "people" => Some(Module::People),
        "tasks" => Some(Module::Tasks),
        _ => None,
    }
}

/// Whether the window takes command keys now: not while File (backstage) or a dialog is open. The
/// menu bar asks the same, so its items are greyed then.
pub fn takes_commands(app: &MailApp) -> bool {
    !app.ui.backstage && app.dialog.is_none()
}

/// What the window runs for a command asked for by a key or from the menu bar: nothing when the
/// command belongs to another module than the one in front (Delete in Calendar never deletes the
/// message still selected in Mail); otherwise the id to run, which for some commands is the
/// window's own (the host does the talking to the server; the search box takes the keyboard).
pub fn window_command(app: &MailApp, id: &'static str) -> Option<&'static str> {
    if module_of(id).is_some_and(|m| m != app.session.view.module) {
        return None;
    }
    Some(match id {
        "sync.request" => "ui.sync",
        "view.search" => "ui.focusSearch",
        other => other,
    })
}

/// The command a key runs now: the first enabled one bound to it that fits the module in front.
pub fn resolve(app: &MailApp, key: &str) -> Option<&'static str> {
    let composing = app.session.compose.is_some();
    let bound = |c: &&mailcraft_engine::CommandSpec| !c.shortcut.is_empty() && c.shortcut.split(" / ").any(|k| k.eq_ignore_ascii_case(key));
    let fits = |c: &&mailcraft_engine::CommandSpec| window_command(app, c.id).is_some();
    let mut hits: Vec<&mailcraft_engine::CommandSpec> = app.session.registry.all().iter().filter(bound).filter(fits).collect();
    // While writing, the Message commands win (Mod+S saves the draft); otherwise they are out.
    hits.sort_by_key(|c| c.id.starts_with("compose.") != composing);
    hits.into_iter().find(|c| (c.enabled)(&app.session).is_none()).map(|c| c.id)
}

/// The same key in the other modules: Delete on a selected event, contact or task.
fn module_delete(app: &MailApp) -> Option<&'static str> {
    match app.session.view.module {
        Module::Calendar if app.session.sel.event.is_some() => Some("calendar.delete"),
        Module::People if app.session.sel.contact.is_some() => Some("people.delete"),
        Module::Tasks if app.session.sel.task.is_some() => Some("tasks.delete"),
        _ => None,
    }
}

/// Command shortcuts when no text field has the keyboard and no dialog is open. Keys with Mod (or
/// function keys) also work while typing, except the ones text editing needs.
pub fn global_shortcuts(app: &mut MailApp, ctx: &egui::Context) {
    if !takes_commands(app) {
        if app.ui.backstage && ctx.input(|i| i.key_pressed(Key::Escape)) {
            app.ui.backstage = false;
        }
        return;
    }
    let typing = ctx.egui_wants_keyboard_input();
    // Esc ends a search, as the ✕ on the bar over the list does (#115). (In the search box
    // itself, the box does it; an open menu takes Esc for itself.)
    if !typing
        && !egui::Popup::is_any_open(ctx)
        && app.session.compose.is_none()
        && app.session.view.module == mailcraft_engine::Module::Mail
        && !app.session.view.search.trim().is_empty()
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape))
    {
        app.search_text.clear();
        let _ = app.run("view.clearSearch", json!({}));
    }
    // The body of a message being written has keys of its own (Ctrl+B, U, Z…), which mean
    // something else everywhere else.
    let in_body = typing && crate::compose::editor::has_focus(ctx);
    let events = ctx.input(|i| i.events.clone());
    for e in events {
        let egui::Event::Key { key, pressed: true, modifiers, .. } = e else { continue };
        let Some(name) = key_name(key) else { continue };
        let function = name.len() > 1 && name.starts_with('F') && name[1..].chars().all(|c| c.is_ascii_digit());
        if typing {
            // The text field keeps plain keys and the editing chords.
            let editing = matches!(
                key,
                Key::A
                    | Key::C
                    | Key::V
                    | Key::X
                    | Key::Z
                    | Key::Y
                    | Key::ArrowLeft
                    | Key::ArrowRight
                    | Key::ArrowUp
                    | Key::ArrowDown
                    | Key::Backspace
                    | Key::Delete
                    | Key::Home
                    | Key::End
            );
            let chord = modifiers.command || modifiers.alt;
            if in_body && modifiers.command && crate::compose::editor::owns_chord(key) {
                continue;
            }
            if !(function || chord && !editing) && !(key == Key::Escape && app.session.compose.is_some()) {
                continue;
            }
        }
        let k = combo(modifiers, name);
        if k == "Delete"
            && let Some(id) = module_delete(app)
        {
            let _ = app.run(id, json!({}));
            continue;
        }
        // In a message that is being read (the pointer last went down in it, or is over it),
        // Select All is about its text, not about the messages of the list.
        if k == "Mod+A"
            && let Some(place) = crate::reading::select_all_params(app, ctx)
        {
            let _ = app.run("ui.selectAllInMessage", place);
            continue;
        }
        if let Some(id) = resolve(app, &k).and_then(|id| window_command(app, id)) {
            let _ = app.run(id, json!({}));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_are_written_like_the_registry() {
        let m = |command, shift, alt| Modifiers { command, shift, alt, ..Default::default() };
        assert_eq!(combo(m(true, false, false), "R"), "Mod+R");
        assert_eq!(combo(m(true, true, false), "R"), "Mod+Shift+R");
        assert_eq!(combo(m(true, false, true), "1"), "Mod+Alt+1");
        assert_eq!(combo(m(false, false, false), "Delete"), "Delete");
        assert_eq!(key_name(Key::F9), Some("F9"));
    }

    #[test]
    fn every_shortcut_in_the_registry_names_a_real_key() {
        let s = mailcraft_engine::Session::empty();
        let names: Vec<&str> = Key::ALL.iter().filter_map(|k| key_name(*k)).collect();
        for c in s.registry.all().iter().filter(|c| !c.shortcut.is_empty()) {
            for k in c.shortcut.split(" / ") {
                let key = k.rsplit('+').next().unwrap();
                assert!(names.contains(&key), "{}: \"{k}\" has no key named {key}", c.id);
                for part in k.split('+').rev().skip(1) {
                    assert!(["Mod", "Shift", "Alt", "Ctrl"].contains(&part), "{}: \"{k}\"", c.id);
                }
            }
        }
    }
}
