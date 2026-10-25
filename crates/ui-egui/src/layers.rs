// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Escape and Enter go to what is on top, and to nothing under it (#406).
//!
//! egui gives every part of the window the same keys of a frame, and the part whose code runs
//! first would get a key, not the part on top. So once a frame, before anything reads the keys,
//! [`resolve`] works out which [`Layer`] is on top, from what is open before anything is drawn.
//! What reads Escape or Enter then asks [`escape`] or [`enter`] for its own layer, and only the
//! layer on top is told yes. A held key is one press (#391, #393): only a new press counts, never
//! its repeats, and a key counts only when it is pressed alone.
//!
//! Escape's repeats are taken out of the input for everybody, and its press is too, so that only
//! this hands it out; except when a menu or a list is open, which egui closes itself on Escape.
//! Send/Receive Progress takes only Escape: Enter goes to what is under it.
//! Enter stays in the input with its repeats, for the text fields and the boxes of several lines
//! that type it; under a question (an agent's, or about unsaved changes) it is taken out, so that
//! a text field under the question doesn't get it.

use egui::{Context, Event, Key, Modifiers, ViewportId, ViewportIdMap};

use crate::MailApp;

/// What can be on top, from the top down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Layer {
    /// An agent's question ([`crate::agent_gate`]).
    Agent,
    /// A window's question about its unsaved changes, by the window's number.
    Unsaved(u64),
    /// An open menu or list: egui closes it on Escape itself.
    Menu,
    /// What a dialog, or a window's form, has open over itself (the recurrence editor, Folder
    /// Size), by the window's number; 0 for the dialog, as [`crate::forms::kit::window`] has it.
    Inner(u64),
    /// A dialog of the main window.
    Dialog,
    /// Send/Receive Progress. It takes only Escape: Enter is for what is under it.
    Progress,
    /// The window in front, where windows are shown inside the main one; in a window of the
    /// system, that window.
    Window(u64),
    /// File.
    File,
    /// The search box, while it has the keyboard.
    Search,
    /// The main window.
    Main,
    /// Nobody: a window's form waits for a question that is asked in the main window.
    Nobody,
}

/// What is open, as far as the keys go: in the main window, or in a window of the system (where
/// only `unsaved`, `menu` and the `window` ones are ever set).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Open {
    pub agent: bool,
    /// A window that asks about its unsaved changes.
    pub unsaved: Option<u64>,
    pub menu: bool,
    pub dialog: bool,
    /// The dialog has something open over itself.
    pub dialog_inner: bool,
    pub progress: bool,
    /// The window in front.
    pub window: Option<u64>,
    /// A window's form has its recurrence editor open: that window's number.
    pub window_inner: Option<u64>,
    /// The window's form waits for a question the main window asks.
    pub window_waits: bool,
    pub file: bool,
    pub search: bool,
}

/// Which layer is on top. A window shown inside the main one floats over File, so it is above it;
/// Send/Receive Progress is drawn over the windows.
pub(crate) fn top(open: &Open) -> Layer {
    if open.agent {
        Layer::Agent
    } else if let Some(window) = open.unsaved {
        Layer::Unsaved(window)
    } else if open.menu {
        Layer::Menu
    } else if open.dialog {
        if open.dialog_inner { Layer::Inner(0) } else { Layer::Dialog }
    } else if open.progress {
        Layer::Progress
    } else if let Some(window) = open.window {
        if let Some(inner) = open.window_inner {
            Layer::Inner(inner)
        } else if open.window_waits {
            Layer::Nobody
        } else {
            Layer::Window(window)
        }
    } else if open.file {
        Layer::File
    } else if open.search {
        Layer::Search
    } else {
        Layer::Main
    }
}

/// The keys of one pass of one viewport, as [`resolve`] found them.
#[derive(Clone, Copy, Debug)]
struct Keys {
    pass: u64,
    top: Layer,
    /// Where Enter goes: what is on top, or under Send/Receive Progress.
    enter_top: Layer,
    /// A new press of the key alone is there and nobody has taken it yet.
    escape: bool,
    enter: bool,
}

fn store() -> egui::Id {
    egui::Id::unique("mailcraft-key-layers")
}

/// A new press of `key` alone (not a held key's repeat).
fn pressed_alone(e: &Event, key: Key) -> bool {
    matches!(e, Event::Key { key: k, pressed: true, repeat: false, modifiers, .. } if *k == key && modifiers.is_none())
}

/// Work out which layer is on top in the viewport being laid out, and keep its keys for it. Once
/// a pass of each viewport, before anything reads Escape or Enter.
pub(crate) fn resolve(ctx: &Context, open: &Open) {
    let enter_top = top(&Open { progress: false, ..*open });
    let top = top(open);
    let (escape, enter) = ctx.input_mut(|i| {
        let escape = i.events.iter().any(|e| pressed_alone(e, Key::Escape));
        let enter = i.events.iter().any(|e| pressed_alone(e, Key::Enter));
        crate::keys::drop_repeats(i, &[Key::Escape]);
        if top != Layer::Menu {
            i.events.retain(|e| !pressed_alone(e, Key::Escape));
        }
        if matches!(top, Layer::Agent | Layer::Unsaved(_)) {
            i.events.retain(
                |e| !matches!(e, Event::Key { key: Key::Enter, pressed: true, modifiers, .. } if modifiers.matches_logically(Modifiers::NONE)),
            );
        }
        (escape, enter)
    });
    let keys = Keys { pass: ctx.cumulative_pass_nr(), top, enter_top, escape, enter };
    let viewport = ctx.viewport_id();
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<ViewportIdMap<Keys>>(store()).insert(viewport, keys);
    });
}

/// What is open in the main window, as far as the keys go.
pub(crate) fn open_in_main(app: &MailApp, ctx: &Context) -> Open {
    Open {
        agent: !app.asking.is_empty(),
        menu: egui::Popup::is_any_open(ctx),
        dialog: app.dialog.is_some(),
        dialog_inner: app.dialog.as_ref().is_some_and(crate::dialogs::Dialog::inner_open),
        progress: app.sync_panel.is_some(),
        file: app.ui.backstage,
        search: ctx.memory(|m| m.has_focus(crate::chrome::search_box_id())),
        ..crate::windows::keys_on_top(app, ctx)
    }
}

/// [`resolve`] for the main window, at the start of its frame. The keys of windows of the system
/// that are gone are forgotten.
pub(crate) fn resolve_main(app: &MailApp, ctx: &Context) {
    let live = crate::windows::viewports(app);
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<ViewportIdMap<Keys>>(store()).retain(|viewport, _| *viewport == ViewportId::ROOT || live.contains(viewport));
    });
    resolve(ctx, &open_in_main(app, ctx));
}

/// Whether `key` was pressed for `layer` this pass; `take` keeps it from what else in the layer
/// asks later.
fn ask(ctx: &Context, layer: Layer, key: Key, take: bool) -> bool {
    let (pass, viewport) = (ctx.cumulative_pass_nr(), ctx.viewport_id());
    ctx.data_mut(|d| {
        let Some(keys) = d.get_temp_mut_or_default::<ViewportIdMap<Keys>>(store()).get_mut(&viewport) else { return false };
        let on_top = if key == Key::Escape { keys.top } else { keys.enter_top };
        if keys.pass != pass || on_top != layer {
            return false;
        }
        let pressed = if key == Key::Escape { &mut keys.escape } else { &mut keys.enter };
        let was = *pressed;
        if take {
            *pressed = false;
        }
        was
    })
}

/// Whether Escape was pressed for `layer`: it is on top, and the press is new.
pub(crate) fn escape(ctx: &Context, layer: Layer) -> bool {
    ask(ctx, layer, Key::Escape, false)
}

/// [`escape`], taken: what else in `layer` asks later in the pass is told no. Ask only when
/// it will be acted on.
pub(crate) fn take_escape(ctx: &Context, layer: Layer) -> bool {
    ask(ctx, layer, Key::Escape, true)
}

/// Whether Enter was pressed for `layer`: it is on top, and the press is new. (The text fields
/// of the layer read Enter themselves, with its repeats.)
pub(crate) fn enter(ctx: &Context, layer: Layer) -> bool {
    ask(ctx, layer, Key::Enter, false)
}

/// [`enter`], taken.
pub(crate) fn take_enter(ctx: &Context, layer: Layer) -> bool {
    ask(ctx, layer, Key::Enter, true)
}
