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
        "mail" | "folder" | "compose" | "format" => Some(Module::Mail),
        "calendar" => Some(Module::Calendar),
        "people" => Some(Module::People),
        "tasks" => Some(Module::Tasks),
        _ => None,
    }
}

/// The command a key runs now: the first enabled one bound to it that fits the module in front.
pub fn resolve(app: &MailApp, key: &str) -> Option<&'static str> {
    let module = app.session.view.module;
    let composing = app.session.compose.is_some();
    let bound = |c: &&mailcraft_engine::CommandSpec| !c.shortcut.is_empty() && c.shortcut.split(" / ").any(|k| k.eq_ignore_ascii_case(key));
    let fits = |c: &&mailcraft_engine::CommandSpec| module_of(c.id).is_none_or(|m| m == module);
    let mut hits: Vec<&mailcraft_engine::CommandSpec> = app.session.registry.all().iter().filter(bound).filter(fits).collect();
    // While writing, the Message commands win (Mod+S saves the draft, Mod+U underlines);
    // otherwise they are out.
    hits.sort_by_key(|c| (c.id.starts_with("compose.") || c.id.starts_with("format.")) != composing);
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
    if app.ui.backstage {
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            app.ui.backstage = false;
        }
        return;
    }
    if app.dialog.is_some() {
        return;
    }
    let typing = ctx.egui_wants_keyboard_input();
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
        if let Some(id) = resolve(app, &k) {
            let id = match id {
                // The host does the talking to the server.
                "sync.request" => "ui.sync",
                // The search box takes the keyboard.
                "view.search" => "ui.focusSearch",
                // Print shows its page and preview first.
                "file.print" => "ui.print",
                // The spelling check shows what it found.
                "review.spelling" => "ui.spelling",
                other => other,
            };
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
