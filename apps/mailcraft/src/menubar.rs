// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The macOS menu bar (#253): the app menu, then File, Edit, View, Folder, Message, Window and
//! Help, made with `muda` from the table in `mailcraft_ui_egui::menubar`. Every item is a command
//! of the registry, run by id as the ribbon runs it; this file only turns the table into native
//! menus, keeps them enabled as the commands are, and hands what is chosen back to the table's
//! `run`. Quit asks first, as the close button does (`craft_quit`).
//!
//! muda's events arrive on the main thread; the handler only queues the item's id and asks for a
//! frame, and [`MenuBar::pump`] runs them in `eframe::App::logic`, on the UI thread. An id that
//! isn't one of ours is ignored.

use std::sync::mpsc::{Receiver, channel};

use egui::Context;
use mailcraft_ui_egui::MailApp;
use mailcraft_ui_egui::i18n::{self, Lang};
use mailcraft_ui_egui::menubar::{self, Action, Chord, Entry, Item};
use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

/// An item's id is this and its place in [`MenuBar::items`].
const ID_PREFIX: &str = "mailcraft.menu.";
/// The most events taken in one frame.
const PER_FRAME: usize = 32;

const LETTERS: [Code; 26] = [
    Code::KeyA,
    Code::KeyB,
    Code::KeyC,
    Code::KeyD,
    Code::KeyE,
    Code::KeyF,
    Code::KeyG,
    Code::KeyH,
    Code::KeyI,
    Code::KeyJ,
    Code::KeyK,
    Code::KeyL,
    Code::KeyM,
    Code::KeyN,
    Code::KeyO,
    Code::KeyP,
    Code::KeyQ,
    Code::KeyR,
    Code::KeyS,
    Code::KeyT,
    Code::KeyU,
    Code::KeyV,
    Code::KeyW,
    Code::KeyX,
    Code::KeyY,
    Code::KeyZ,
];
const DIGITS: [Code; 10] =
    [Code::Digit0, Code::Digit1, Code::Digit2, Code::Digit3, Code::Digit4, Code::Digit5, Code::Digit6, Code::Digit7, Code::Digit8, Code::Digit9];

/// The menu bar and what it needs between frames.
pub struct MenuBar {
    /// The installed menu: muda's items must outlive the native menu that shows them.
    _menu: Menu,
    /// Every item that runs something, with what it runs and whether it was last shown enabled.
    items: Vec<(MenuItem, Action, bool)>,
    events: Receiver<String>,
    lang: Lang,
}

/// Make the menu bar and put it in place of winit's menu. Call once, from the eframe creator
/// (winit builds its own menu before that and would replace one made earlier).
pub fn install(ctx: &Context, app: &MailApp) -> Result<MenuBar, Box<dyn std::error::Error + Send + Sync>> {
    let (tx, events) = channel::<String>();
    let ctx = ctx.clone();
    let (menu, items) = build(app)?;
    menu.init_for_nsapp();
    // One window, no tabs: AppKit would otherwise add Show Tab Bar and Show All Tabs to the menus,
    // greyed. (Called on the main thread, from the eframe creator; elsewhere it is left alone.)
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        objc2_app_kit::NSWindow::setAllowsAutomaticWindowTabbing(false, mtm);
    }
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        // Quit asks the window to close, so a message being written is asked about first.
        if !craft_quit::on_menu_event(&ctx, event.id.as_ref()) {
            let _ = tx.send(event.id.as_ref().to_string());
        }
        ctx.request_repaint();
    }));
    Ok(MenuBar { _menu: menu, items, events, lang: i18n::current() })
}

impl MenuBar {
    /// Once a frame, from `logic`: take the items chosen and run them, and keep each item
    /// enabled as its command is.
    pub fn pump(&mut self, app: &mut MailApp, ctx: &Context) {
        if i18n::current() != self.lang {
            self.lang = i18n::current();
            match build(app) {
                Ok((menu, items)) => {
                    menu.init_for_nsapp();
                    self._menu = menu;
                    self.items = items;
                }
                Err(e) => log::warn!("the menu bar couldn't be made again in the new language: {e}"),
            }
        }
        for id in self.events.try_iter().take(PER_FRAME) {
            let action = id.strip_prefix(ID_PREFIX).and_then(|n| n.parse::<usize>().ok()).and_then(|n| self.items.get(n)).map(|(_, a, _)| a.clone());
            let Some(action) = action else { continue };
            if menubar::enabled(app, &action) {
                menubar::run(app, &action, clipboard_text);
                ctx.request_repaint();
            }
        }
        for (item, action, shown) in &mut self.items {
            let on = menubar::enabled(app, action);
            if on != *shown {
                item.set_enabled(on);
                *shown = on;
            }
        }
    }
}

/// The text on the clipboard, for Paste (none is an empty paste).
fn clipboard_text() -> String {
    arboard::Clipboard::new().and_then(|mut board| board.get_text()).unwrap_or_default()
}

type Built = (Menu, Vec<(MenuItem, Action, bool)>);

fn build(app: &MailApp) -> Result<Built, Box<dyn std::error::Error + Send + Sync>> {
    let words = menubar::app_text();
    let mut items: Vec<(MenuItem, Action, bool)> = Vec::new();
    let menu = Menu::new();

    // MailCraft: About, Settings, the system's, and Quit (which asks first).
    let app_menu = Submenu::new("MailCraft", true);
    app_menu.append(&PredefinedMenuItem::about(Some(&words.about), None))?;
    app_menu.append(&PredefinedMenuItem::separator())?;
    for it in menubar::app_items() {
        app_menu.append(&native(app, &it, &mut items))?;
    }
    app_menu.append_items(&[
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::services(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::hide(Some(&words.hide)),
        &PredefinedMenuItem::hide_others(None),
        &PredefinedMenuItem::show_all(None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(craft_quit::QUIT_MENU_ID, &words.quit, true, accelerator(&Chord { shift: false, alt: false, key: "Q".into() })),
    ])?;
    menu.append(&app_menu)?;

    let blocks = menubar::menus(app);
    let help = blocks.len().saturating_sub(1);
    for (n, block) in blocks.iter().enumerate() {
        // The system's Window menu stands before Help.
        if n == help {
            let window = Submenu::new(menubar::window_title(), true);
            window.append_items(&[&PredefinedMenuItem::minimize(Some(&words.minimize)), &PredefinedMenuItem::maximize(Some(&words.zoom))])?;
            menu.append(&window)?;
            window.set_as_windows_menu_for_nsapp();
        }
        let sub = Submenu::new(escape(&block.title), true);
        for entry in &block.entries {
            match entry {
                Entry::Separator => sub.append(&PredefinedMenuItem::separator())?,
                Entry::Item(it) => sub.append(&native(app, it, &mut items))?,
            }
        }
        menu.append(&sub)?;
        if n == help {
            sub.set_as_help_menu_for_nsapp();
        }
    }
    Ok((menu, items))
}

/// A native item for `it`, remembered in `items` under the id it carries.
fn native(app: &MailApp, it: &Item, items: &mut Vec<(MenuItem, Action, bool)>) -> MenuItem {
    let on = menubar::enabled(app, &it.action);
    let item = MenuItem::with_id(format!("{ID_PREFIX}{}", items.len()), escape(&it.label), on, it.chord.as_ref().and_then(accelerator));
    items.push((item.clone(), it.action.clone(), on));
    item
}

/// muda reads `&` in a label as a mnemonic; a label's own is doubled.
fn escape(label: &str) -> String {
    label.replace('&', "&&")
}

/// ⌘ and the chord's key, as muda names them.
fn accelerator(c: &Chord) -> Option<Accelerator> {
    let mut chars = c.key.chars();
    let code = match (chars.next(), chars.next()) {
        (Some(','), None) => Code::Comma,
        (Some(ch), None) if ch.is_ascii_uppercase() => *LETTERS.get(usize::try_from(u32::from(ch) - u32::from('A')).ok()?)?,
        (Some(ch), None) if ch.is_ascii_digit() => *DIGITS.get(usize::try_from(u32::from(ch) - u32::from('0')).ok()?)?,
        _ if c.key == "Enter" => Code::Enter,
        _ => return None,
    };
    let mut modifiers = Modifiers::META;
    if c.shift {
        modifiers |= Modifiers::SHIFT;
    }
    if c.alt {
        modifiers |= Modifiers::ALT;
    }
    Some(Accelerator::new(modifiers, code))
}
