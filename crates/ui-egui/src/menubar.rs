// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The menus of the macOS menu bar (#253), as data: which commands each menu holds, in what
//! order, with the label and ⌘ key each shows and whether it can run now. The desktop app
//! (`apps/mailcraft/src/menubar.rs`, macOS only) turns this into native menus and hands the item
//! chosen back to [`run`]; nothing here touches the operating system, so the table is tested on
//! every platform.
//!
//! Every item is a command from the registry, run by id as the ribbon runs it (`MailApp::run`),
//! labelled with the registry's own label. The Edit menu's Undo, Redo, Cut, Copy, Paste and
//! Select All are not commands: they are the key presses a text field already understands, so
//! they work in whatever has the focus, the message being written included.
//!
//! A menu item's key is a key equivalent: AppKit takes the key press before the window sees it.
//! So a key is shown only where the item may have it: never one a text field needs for editing
//! (⌘A, ⌘C, ⌘X, ⌘V, ⌘Z, ⌘Y) or ⌘Q, W, H and M, which the system's own items have; and never a key
//! without ⌘ (Delete, Escape, the arrows), which a text field needs as well. Those items still
//! work with their keys, as before: the window handles them.

use std::collections::HashSet;

use egui::{Event, Key, Modifiers};
use serde_json::Value;

use crate::MailApp;

/// A key equivalent: ⌘ and the key, with ⇧ and ⌥ where named. `key` is one letter or digit
/// (upper case), `Enter` or `,`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub shift: bool,
    pub alt: bool,
    pub key: String,
}

impl Chord {
    fn new(shift: bool, alt: bool, key: &str) -> Self {
        Chord { shift, alt, key: key.to_string() }
    }
}

/// What an item does.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// A command of the registry (or a `ui.*` command), with its params as JSON.
    Command { id: &'static str, params: &'static str },
    /// One of the Edit menu's standard items.
    Edit(EditKey),
}

/// The Edit menu's standard items, which act on the focused text field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKey {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

/// An item of the menus that is not only a registry command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Own {
    Edit(EditKey),
    Info,
    Options,
    About,
    HelpTab,
    Settings,
}

impl Own {
    fn label(self) -> &'static str {
        match self {
            Own::Edit(EditKey::Undo) => "Undo",
            Own::Edit(EditKey::Redo) => "Redo",
            Own::Edit(EditKey::Cut) => "Cut",
            Own::Edit(EditKey::Copy) => "Copy",
            Own::Edit(EditKey::Paste) => "Paste",
            Own::Edit(EditKey::SelectAll) => "Select All",
            Own::Info => "Info",
            Own::Options => "Options",
            Own::About => "About MailCraft",
            Own::HelpTab => "Help",
            Own::Settings => "Settings…",
        }
    }

    fn action(self) -> Action {
        match self {
            Own::Edit(k) => Action::Edit(k),
            Own::Info => Action::Command { id: "ui.backstage", params: r#"{"value": true, "page": "info"}"# },
            Own::Options | Own::Settings => Action::Command { id: "ui.backstage", params: r#"{"value": true, "page": "options"}"# },
            Own::About => Action::Command { id: "ui.backstage", params: r#"{"value": true, "page": "about"}"# },
            Own::HelpTab => Action::Command { id: "ui.tab", params: r#"{"tab": "Help"}"# },
        }
    }

    fn chord(self) -> Option<Chord> {
        Some(match self {
            Own::Edit(EditKey::Undo) => Chord::new(false, false, "Z"),
            Own::Edit(EditKey::Redo) => Chord::new(true, false, "Z"),
            Own::Edit(EditKey::Cut) => Chord::new(false, false, "X"),
            Own::Edit(EditKey::Copy) => Chord::new(false, false, "C"),
            Own::Edit(EditKey::Paste) => Chord::new(false, false, "V"),
            Own::Edit(EditKey::SelectAll) => Chord::new(false, false, "A"),
            Own::Settings => Chord::new(false, false, ","),
            _ => return None,
        })
    }
}

/// One line of a menu's table.
#[derive(Clone, Copy)]
enum Row {
    /// A command of the registry.
    Cmd(&'static str),
    Own(Own),
    Sep,
}

use Row::{Cmd, Own as O, Sep};

const FILE: &[Row] = &[
    Cmd("mail.new"),
    Cmd("calendar.new"),
    Cmd("calendar.newMeeting"),
    Cmd("people.new"),
    Cmd("tasks.new"),
    Sep,
    Cmd("file.new"),
    Cmd("file.open"),
    Cmd("file.save"),
    Cmd("file.saveAs"),
    Sep,
    Cmd("file.import"),
    Cmd("file.exportMessage"),
    Cmd("file.exportFolder"),
    Cmd("file.exportCalendar"),
    Cmd("file.exportContacts"),
    Sep,
    Cmd("account.add"),
    O(Own::Info),
    O(Own::Options),
];

const EDIT: &[Row] = &[
    O(Own::Edit(EditKey::Undo)),
    O(Own::Edit(EditKey::Redo)),
    Sep,
    O(Own::Edit(EditKey::Cut)),
    O(Own::Edit(EditKey::Copy)),
    O(Own::Edit(EditKey::Paste)),
    O(Own::Edit(EditKey::SelectAll)),
    Sep,
    Cmd("view.search"),
    Cmd("view.clearSearch"),
];

const VIEW: &[Row] = &[
    Cmd("view.mail"),
    Cmd("view.calendar"),
    Cmd("view.people"),
    Cmd("view.tasks"),
    Sep,
    Cmd("view.readingPane"),
    Cmd("view.folderPane"),
    Cmd("view.conversations"),
    Cmd("view.reverseSort"),
    Sep,
    Cmd("view.day"),
    Cmd("view.workWeek"),
    Cmd("view.week"),
    Cmd("view.month"),
    Cmd("view.today"),
    Cmd("view.goToDate"),
    Sep,
    Cmd("view.reminders"),
    Cmd("view.reset"),
];

const FOLDER: &[Row] = &[
    Cmd("folder.new"),
    Cmd("folder.rename"),
    Cmd("folder.move"),
    Cmd("folder.delete"),
    Cmd("folder.favorite"),
    Sep,
    Cmd("folder.select"),
    Cmd("folder.empty"),
    Cmd("mail.markAllRead"),
    Cmd("rules.run"),
    Sep,
    Cmd("sync.request"),
];

const MESSAGE: &[Row] = &[
    Cmd("mail.reply"),
    Cmd("mail.replyAll"),
    Cmd("mail.forward"),
    Sep,
    Cmd("mail.delete"),
    Cmd("mail.archive"),
    Cmd("mail.move"),
    Cmd("mail.copy"),
    Sep,
    Cmd("mail.markRead"),
    Cmd("mail.markUnread"),
    Cmd("mail.toggleFlag"),
    Cmd("mail.clearFlag"),
    Cmd("mail.complete"),
    Cmd("mail.categorize"),
    Cmd("mail.pin"),
    Sep,
    Cmd("mail.junk"),
    Cmd("mail.notJunk"),
    Sep,
    Cmd("compose.send"),
    Cmd("compose.saveDraft"),
    Cmd("compose.attach"),
    Cmd("compose.checkNames"),
];

const HELP: &[Row] = &[O(Own::HelpTab), O(Own::About)];

/// The menus between the app menu and Window, with their tables.
const TABLES: &[(Menu, &[Row])] = &[(Menu::File, FILE), (Menu::Edit, EDIT), (Menu::View, VIEW), (Menu::Folder, FOLDER), (Menu::Message, MESSAGE)];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    File,
    Edit,
    View,
    Folder,
    Message,
}

impl Menu {
    fn title(self) -> &'static str {
        match self {
            Menu::File => "File",
            Menu::Edit => "Edit",
            Menu::View => "View",
            Menu::Folder => "Folder",
            Menu::Message => "Message",
        }
    }
}

/// One item of a menu.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub label: String,
    pub chord: Option<Chord>,
    pub action: Action,
}

/// A line of a menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Item(Item),
    Separator,
}

/// A menu of the menu bar.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuBlock {
    pub title: String,
    pub entries: Vec<Entry>,
}

/// The title of the Window menu, which the host fills with the system's own items.
pub fn window_title() -> String {
    tl!("Window").to_string()
}

/// The items the app menu has besides the system's own (About, Hide, Quit).
pub fn app_items() -> Vec<Item> {
    vec![item(Own::Settings)]
}

/// The words of the app menu and the Window menu, whose items the system provides.
pub struct AppText {
    pub about: String,
    pub hide: String,
    pub quit: String,
    pub minimize: String,
    pub zoom: String,
}

pub fn app_text() -> AppText {
    AppText {
        about: tl!("About MailCraft").to_string(),
        hide: tl!("Hide MailCraft").to_string(),
        quit: tl!("Quit MailCraft").to_string(),
        minimize: tl!("Minimize").to_string(),
        zoom: tl!("Zoom").to_string(),
    }
}

fn item(own: Own) -> Item {
    Item { label: tl!(own.label()).to_string(), chord: own.chord(), action: own.action() }
}

/// The key a registry shortcut (`Mod+Shift+R`, `Delete / Mod+D`) gives an item, when it may have
/// one (see the module's text).
pub fn chord(shortcut: &str) -> Option<Chord> {
    shortcut.split(" / ").find_map(|alternative| {
        let mut parts: Vec<&str> = alternative.split('+').collect();
        let key = parts.pop()?;
        if parts.first() != Some(&"Mod") {
            return None;
        }
        let (mut shift, mut alt) = (false, false);
        for modifier in parts.iter().skip(1) {
            match *modifier {
                "Shift" => shift = true,
                "Alt" => alt = true,
                _ => return None,
            }
        }
        let mut chars = key.chars();
        let single = matches!((chars.next(), chars.next()), (Some(c), None) if c.is_ascii_alphanumeric());
        if !single && key != "Enter" {
            return None;
        }
        // The keys a text field, or the system's own items, have.
        if !alt && matches!(key, "A" | "C" | "X" | "V" | "Z" | "Y" | "Q" | "W" | "H" | "M") {
            return None;
        }
        Some(Chord::new(shift, alt, key))
    })
}

/// The menus after the app menu: File, Edit, View, Folder, Message, and Help last. Window (the
/// system's items) goes between Message and Help ([`window_title`]). A command the registry
/// doesn't have is left out.
pub fn menus(app: &MailApp) -> Vec<MenuBlock> {
    let mut taken: HashSet<Chord> = HashSet::new();
    let mut blocks: Vec<MenuBlock> = TABLES.iter().map(|(menu, rows)| block(app, tl!(menu.title()), rows, &mut taken)).collect();
    blocks.push(block(app, tl!("Help"), HELP, &mut taken));
    blocks
}

fn block(app: &MailApp, title: &str, rows: &[Row], taken: &mut HashSet<Chord>) -> MenuBlock {
    let mut entries: Vec<Entry> = Vec::new();
    for row in rows {
        let found = match row {
            Row::Sep => {
                // (Never first, never twice in a row.)
                if !matches!(entries.last(), None | Some(Entry::Separator)) {
                    entries.push(Entry::Separator);
                }
                continue;
            }
            Row::Own(own) => Some(item(*own)),
            Row::Cmd(id) => app.session.registry.get(id).map(|spec| Item {
                label: tl!(spec.label).to_string(),
                chord: chord(spec.shortcut),
                action: Action::Command { id: spec.id, params: "{}" },
            }),
        };
        if let Some(mut it) = found {
            // A key goes to one item: the first to ask for it.
            if let Some(c) = it.chord.take() {
                it.chord = taken.insert(c.clone()).then_some(c);
            }
            entries.push(Entry::Item(it));
        }
    }
    while matches!(entries.last(), Some(Entry::Separator)) {
        entries.pop();
    }
    MenuBlock { title: title.to_string(), entries }
}

/// Whether the item can be chosen now. The Edit menu's items always can: the text field that
/// has the focus decides what they do.
pub fn enabled(app: &MailApp, action: &Action) -> bool {
    match action {
        Action::Command { id, .. } => app.enabled(id),
        Action::Edit(_) => true,
    }
}

/// Choose an item: a command runs as the ribbon runs it (a failure shows in the status bar); an
/// Edit item becomes the input a text field takes on its next frame. `clipboard_text` is asked
/// for only by Paste.
pub fn run(app: &mut MailApp, action: &Action, clipboard_text: impl FnOnce() -> String) {
    match action {
        Action::Command { id, params } => {
            let params = serde_json::from_str::<Value>(params).ok().filter(Value::is_object).unwrap_or_else(|| serde_json::json!({}));
            let _ = app.run(id, params);
        }
        Action::Edit(key) => {
            // ⌘ as the window reports it on macOS.
            let cmd = Modifiers { mac_cmd: true, command: true, ..Modifiers::NONE };
            let press = |key: Key, modifiers: Modifiers| {
                vec![
                    Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers },
                    Event::Key { key, physical_key: Some(key), pressed: false, repeat: false, modifiers },
                ]
            };
            let events: Vec<Event> = match key {
                EditKey::Cut => vec![Event::Cut],
                EditKey::Copy => vec![Event::Copy],
                EditKey::Paste => vec![Event::Paste(clipboard_text())],
                EditKey::Undo => press(Key::Z, cmd),
                EditKey::Redo => press(Key::Z, Modifiers { shift: true, ..cmd }),
                EditKey::SelectAll => press(Key::A, cmd),
            };
            // (Bounded: the queue is drained a frame at a time.)
            if app.synthetic.len() < 64 {
                app.synthetic.extend(events);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use mailcraft_engine::Session;
    use mailcraft_model::Mailbox;

    use super::*;
    use crate::Services;

    fn app() -> MailApp {
        MailApp::new(Session::new(Mailbox::default()), Services::default())
    }

    fn items(block: &MenuBlock) -> impl Iterator<Item = &Item> {
        block.entries.iter().filter_map(|e| if let Entry::Item(i) = e { Some(i) } else { None })
    }

    #[test]
    fn every_listed_command_exists_and_none_is_left_out() {
        let app = app();
        let tables: Vec<&[Row]> = TABLES.iter().map(|t| t.1).chain([HELP]).collect();
        for rows in &tables {
            for row in *rows {
                if let Row::Cmd(id) = row {
                    assert!(app.session.registry.get(id).is_some(), "{id} is not a command");
                }
            }
        }
        let wanted: usize = tables.iter().flat_map(|r| r.iter()).filter(|r| !matches!(r, Row::Sep)).count();
        let built: usize = menus(&app).iter().map(|m| items(m).count()).sum();
        assert_eq!(wanted, built);
        // The `ui.*` commands the own items run exist too, with params that read.
        for own in [Own::Info, Own::Options, Own::About, Own::HelpTab, Own::Settings] {
            let Action::Command { id, params } = own.action() else { panic!("{own:?} is not a command") };
            assert!(app.enabled(id), "{id} is not a command");
            assert!(serde_json::from_str::<Value>(params).is_ok_and(|v| v.is_object()), "{params}");
        }
    }

    #[test]
    fn no_menu_is_empty_and_separators_only_divide() {
        let app = app();
        let menus = menus(&app);
        assert_eq!(menus.len(), TABLES.len() + 1);
        for m in &menus {
            assert!(!m.title.is_empty());
            assert!(items(m).count() > 0, "{} is empty", m.title);
            assert!(!matches!(m.entries.first(), Some(Entry::Separator)));
            assert!(!matches!(m.entries.last(), Some(Entry::Separator)));
            assert!(m.entries.windows(2).all(|w| !matches!(w, [Entry::Separator, Entry::Separator])), "{}", m.title);
            assert!(items(m).all(|i| !i.label.is_empty()));
        }
        assert!(!window_title().is_empty());
        assert!(!app_items().is_empty());
        let t = app_text();
        assert!([t.about, t.hide, t.quit, t.minimize, t.zoom].iter().all(|s| !s.is_empty()));
    }

    #[test]
    fn a_key_goes_to_one_item_and_none_is_one_a_text_field_needs() {
        let app = app();
        let mut seen = HashSet::new();
        for i in menus(&app).iter().flat_map(items) {
            if let Some(c) = &i.chord {
                assert!(seen.insert(c.clone()), "{c:?} twice");
                assert!(c.key == "Enter" || c.key.chars().count() == 1);
            }
        }
        assert!(seen.contains(&Chord::new(false, false, "Z")) && seen.contains(&Chord::new(false, false, "V")));
        assert!(chord("Delete").is_none() && chord("Escape").is_none() && chord("F9").is_none());
        assert!(chord("Mod+Q").is_none() && chord("Mod+Y").is_none() && chord("Mod+A").is_none());
        assert_eq!(chord("Delete / Mod+D"), Some(Chord::new(false, false, "D")));
        assert_eq!(chord("Mod+Shift+R"), Some(Chord::new(true, false, "R")));
        assert_eq!(chord("Mod+Alt+1"), Some(Chord::new(false, true, "1")));
        assert_eq!(chord("Mod+Enter / Alt+S"), Some(Chord::new(false, false, "Enter")));
        assert!(chord("").is_none() && chord("Mod+").is_none() && chord("Mod+Foo+R").is_none());
    }

    #[test]
    fn commands_are_enabled_from_the_registry_and_unknown_ones_are_refused() {
        let mut app = app();
        // Nothing is selected in an empty mailbox, so Reply can't run; New Email can.
        assert!(!enabled(&app, &Action::Command { id: "mail.reply", params: "{}" }));
        assert!(enabled(&app, &Action::Command { id: "mail.new", params: "{}" }));
        assert!(!enabled(&app, &Action::Command { id: "no.such.command", params: "{}" }));
        assert!(enabled(&app, &Action::Edit(EditKey::Copy)));
        // Running something unknown, with junk params, does nothing and does not panic.
        run(&mut app, &Action::Command { id: "no.such.command", params: "{" }, String::new);
    }

    #[test]
    fn edit_items_become_the_input_a_text_field_takes() {
        let mut app = app();
        run(&mut app, &Action::Edit(EditKey::Paste), || "pasted".to_string());
        run(&mut app, &Action::Edit(EditKey::SelectAll), String::new);
        assert!(app.synthetic.iter().any(|e| matches!(e, Event::Paste(t) if t == "pasted")));
        assert!(app.synthetic.iter().any(|e| matches!(e, Event::Key { key: Key::A, pressed: true, modifiers, .. } if modifiers.command)));
        for _ in 0..200 {
            run(&mut app, &Action::Edit(EditKey::Copy), String::new);
        }
        assert!(app.synthetic.len() <= 70);
    }
}
