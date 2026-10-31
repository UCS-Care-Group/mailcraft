// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Keyboard shortcuts: the registry's `shortcut` fields, run when no text field has the keyboard.
//!
//! Escape and Enter go to one place, and to nothing under it (#406). egui gives every part of a
//! window the same keys of a pass, so once a pass of each viewport, before anything reads them,
//! [`gate`] works out whose a new press of either is, from what egui knows: a modal layer on top
//! takes it; else an open list keeps Escape for egui to close it; else Send/Receive Progress
//! takes Escape; else the widget with the keyboard (a text field, a button) has it, in its window;
//! else the active window, the one last clicked or typed in, as with windows of the system.
//! The press is kept for that layer and taken out of the input, so that only what asks for it
//! ([`escape`], [`enter`]) by its layer gets it; Enter stays in the input for a widget that has
//! the keyboard. A held key is one press (#391, #393): its repeats are nobody's, except Enter's
//! in the text field it is typing new lines in. A key counts only when it is pressed alone.

use egui::{Event, Key, LayerId, Modifiers, Order, ViewportId, ViewportIdMap};
use mailcraft_engine::Module;
use serde_json::json;

use crate::MailApp;

/// The main window's layer: its panels are drawn on egui's background. (In a window of the
/// system, that window.)
pub(crate) fn main_layer() -> LayerId {
    LayerId::background()
}

/// The layer of a dialog of the main window ([`crate::dialogs::window`]).
pub(crate) fn dialog_layer() -> LayerId {
    LayerId::new(Order::Middle, egui::Id::unique("dialog"))
}

/// The layer of the `egui::Modal` with this id.
pub(crate) fn modal_layer(id: egui::Id) -> LayerId {
    LayerId::new(Order::Foreground, id)
}

/// Whose a new press of Escape or Enter is: a layer, and the widget in it that had the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Owner {
    layer: LayerId,
    field: Option<egui::Id>,
}

/// What the gate keeps of one viewport from one pass to the next.
#[derive(Clone, Debug, Default)]
struct Chain {
    /// A held Escape or Enter, and the widget that had the keyboard when it went down (none when
    /// a list was open then: the press was the list's, not the widget's).
    held: Option<(Key, Option<egui::Id>)>,
    /// The windows in the order they were last made active, the latest first; the main window
    /// (or the window of the system) is always there under them.
    active: Vec<LayerId>,
    /// The windows shown, the lowest first, and the widget with the keyboard, at the last gate.
    shown: Vec<LayerId>,
    focus: Option<egui::Id>,
    /// The modal layers that came up, the lowest first, each with what had the keyboard then.
    modals: Vec<(LayerId, Option<egui::Id>)>,
    /// What was given the keyboard back, while the Escape that closed the modal layer (or the
    /// list under a field) is held.
    back: Option<egui::Id>,
    /// This pass's new presses, whose they are until taken.
    pass: u64,
    escape: Option<Owner>,
    enter: Option<Owner>,
    /// This pass's new Escape went to the list that is open (egui closes it on the key itself).
    list_escape: bool,
    /// The layers this pass's presses were given to, and whether anything of the layer asked for
    /// the key: if nothing did and the layer wasn't drawn, the press is made again.
    sent: Vec<(Key, LayerId, bool)>,
    /// Enter, pressed in the pass the keyboard went back to a widget, made again in the next.
    later: Vec<Key>,
    /// The widgets that keep the keyboard on Escape and act on it themselves: those that asked
    /// for it with [`take_escape_in`] (the search box).
    keepers: Vec<egui::Id>,
    /// What the keyboard went back to in the pass before.
    given: Option<egui::Id>,
}

/// Most windows the gate remembers the order of.
const REMEMBERED: usize = 32;

fn store() -> egui::Id {
    egui::Id::unique("mailcraft-key-chain")
}

/// A press (or a repeat) of `key` alone.
fn alone(e: &Event, key: Key) -> bool {
    matches!(e, Event::Key { key: k, pressed: true, modifiers, .. } if *k == key && modifiers.is_none())
}

/// The window a layer belongs to: a sublayer's parent; the background is the main window's; a
/// window is a visible layer of the middle order (a dialog's, or one inside the main window).
/// Anything else (a list, a tooltip, Progress, the shade under a dialog, Reminders) is nobody's
/// window.
fn window_of(ctx: &egui::Context, layer: LayerId) -> Option<LayerId> {
    let layer = ctx.memory(|m| m.areas().parent_layer(layer)).unwrap_or(layer);
    if layer.order == Order::Background {
        Some(main_layer())
    } else if is_window(ctx, layer) {
        Some(layer)
    } else {
        None
    }
}

/// Reminders is no window here: on the desktop it is a window of the system that asks for
/// attention and leaves the keyboard where it is, and inside the main one (in a browser) it reads
/// neither Escape nor Enter, so a click on it doesn't take them from the window that has them.
fn is_window(ctx: &egui::Context, layer: LayerId) -> bool {
    layer.order == Order::Middle
        && layer != crate::dialogs::scrim_layer()
        && layer != crate::reminders::embedded_layer()
        && ctx.memory(|m| m.areas().is_visible(&layer) && m.areas().parent_layer(layer).is_none())
}

/// The layer of the widget `id`, as egui last laid it out. (egui's `read_response` also takes
/// the keyboard away from a widget pressed away from, before the widget is laid out and does so
/// itself: what reads whether it has the keyboard before that, a list under a field, would be
/// told no. Not here.)
fn layer_of(ctx: &egui::Context, id: egui::Id) -> Option<LayerId> {
    let was = ctx.options(|o| o.input_options.surrender_focus_on);
    ctx.options_mut(|o| o.input_options.surrender_focus_on = egui::SurrenderFocusOn::Never);
    let layer = ctx.read_response(id).map(|r| r.layer_id);
    ctx.options_mut(|o| o.input_options.surrender_focus_on = was);
    layer
}

/// The list (a combo box's, a menu) that is open, as egui last drew it: a popup's area has the
/// popup's own id. The recipient suggestions under the field `focus` (egui's `CompletionPopup`)
/// are a list too: egui keeps no note of them among its popups, and their area is the field's id
/// with "completion_popup".
fn open_list(ctx: &egui::Context, focus: Option<egui::Id>) -> Option<LayerId> {
    if let Some(list) = focus.map(suggestions).filter(|l| ctx.memory(|m| m.areas().is_visible(l))) {
        return Some(list);
    }
    if !egui::Popup::is_any_open(ctx) {
        return None;
    }
    let shown: Vec<LayerId> = ctx.memory(|m| m.layer_ids().filter(|l| l.order == Order::Foreground && m.areas().is_visible(l)).collect());
    shown.into_iter().rev().find(|l| egui::Popup::is_id_open(ctx, l.id))
}

/// The layer of the recipient suggestions under the field `id` (egui's `CompletionPopup`).
fn suggestions(id: egui::Id) -> LayerId {
    LayerId::new(Order::Foreground, id.with("completion_popup"))
}

/// Whether a widget types the new lines of a held Enter: a text field (egui's), or the body of a
/// message being written.
fn types(ctx: &egui::Context, id: egui::Id) -> bool {
    egui::text_edit::TextEditState::load(ctx, id).is_some() || id == crate::compose::editor::id()
}

/// Which window is active, from what happened since the last gate: a window that came up
/// without a press is (it opened, the lowest of several first; or a command raised it to the
/// front), as a new window of the system is; the window the keyboard moved to is; a press makes the one under the pointer active, by
/// what egui last drew, which is what the owner saw (a list of a window is that window's, the main
/// window's panels are the main window's). A window that is gone drops out, so the one active
/// before it is again.
fn activate(ctx: &egui::Context, chain: &mut Chain, focus: Option<egui::Id>) {
    let make = |active: &mut Vec<LayerId>, layer: LayerId| {
        active.retain(|l| *l != layer);
        active.insert(0, layer);
    };
    let shown: Vec<LayerId> = ctx.memory(|m| m.layer_ids().collect::<Vec<_>>()).into_iter().filter(|l| is_window(ctx, *l)).collect();
    let raised = shown.last().filter(|front| chain.shown.last() != Some(front) && chain.shown.last().is_none_or(|was| shown.contains(was)));
    for window in shown.iter().filter(|l| !chain.shown.contains(l)).chain(raised) {
        make(&mut chain.active, *window);
    }
    chain.shown = shown;
    if focus != chain.focus
        && let Some(window) = focus.and_then(|id| layer_of(ctx, id)).and_then(|l| window_of(ctx, l))
    {
        make(&mut chain.active, window);
    }
    chain.focus = focus;
    let presses: Vec<egui::Pos2> = ctx
        .input(|i| i.events.iter().filter_map(|e| if let Event::PointerButton { pos, pressed: true, .. } = e { Some(*pos) } else { None }).collect());
    for pos in presses {
        if let Some(window) = ctx.layer_id_at(pos).map_or(Some(main_layer()), |l| window_of(ctx, l)) {
            make(&mut chain.active, window);
        }
    }
    chain.active.retain(|l| *l == main_layer() || is_window(ctx, *l));
    chain.active.truncate(REMEMBERED);
}

/// The keyboard goes back to what had it when a modal layer came up, once that layer is gone,
/// unless something has taken it since. egui's layer outlives an `egui::Modal` by a pass, so
/// this waits for egui to say so. While the Escape that closed it is held, it is given again
/// when a repeat takes it away (egui lets a widget keep the keyboard on Escape only from its
/// second pass with it). On a new press of Escape it goes back only to a widget that keeps the
/// keyboard on Escape, for the press; from any other egui takes it away on Escape, a pass later.
/// egui takes it from that widget too, at the start of the pass after (it lets a widget keep it
/// only from its second pass with it): then it is given again, for the press. Whether it went
/// back in this pass.
fn give_back(ctx: &egui::Context, chain: &mut Chain, modal: Option<LayerId>, mut focus: Option<egui::Id>) -> bool {
    let closed: Vec<(LayerId, Option<egui::Id>)> = match modal {
        Some(top) => match chain.modals.iter().position(|(layer, _)| *layer == top) {
            Some(at) => chain.modals.drain(at + 1..).collect(),
            None => {
                chain.modals.push((top, focus));
                if chain.modals.len() > REMEMBERED {
                    chain.modals.remove(0);
                }
                Vec::new()
            }
        },
        None => std::mem::take(&mut chain.modals),
    };
    // A button of the dialog that closed (OK, reached with Tab) has the keyboard until egui sees,
    // at the end of this pass, that it is gone: it is nothing's to keep.
    if let Some(id) = focus
        && layer_of(ctx, id).is_some_and(|l| closed.iter().any(|(layer, _)| *layer == l))
    {
        ctx.memory_mut(|m| m.surrender_focus(id));
        focus = None;
    }
    if let Some((_, id)) = closed.first() {
        chain.back = *id;
    }
    let escape = ctx.input(|i| i.events.iter().any(|e| alone(e, Key::Escape) && matches!(e, Event::Key { repeat: false, .. })));
    let just = chain.given.take();
    let keeps = |id: egui::Id| chain.keepers.contains(&id);
    let given = match (chain.back, focus) {
        (Some(id), None) if escape && !keeps(id) => {
            chain.back = None;
            None
        }
        (Some(id), None) => Some(id),
        (Some(id), Some(other)) if other != id => {
            chain.back = None;
            None
        }
        (None, None) if escape => just.filter(|id| keeps(*id)),
        _ => None,
    };
    if let Some(id) = given {
        ctx.memory_mut(|m| m.request_focus(id));
        chain.given = Some(id);
    }
    if !ctx.input(|i| i.key_down(Key::Escape)) {
        chain.back = None;
    }
    given.is_some()
}

/// Once a pass of each viewport, before anything reads Escape or Enter: works out whose a new
/// press of either is and keeps it for that one, and takes the keys out of the input that are
/// nobody else's. `blocked`: the viewport waits for a question asked in another one (an agent's,
/// in the main window), so its Escape and Enter are nobody's.
pub(crate) fn gate(ctx: &egui::Context, blocked: bool) {
    let viewport = ctx.viewport_id();
    let mut chain = ctx.data_mut(|d| d.get_temp_mut_or_default::<ViewportIdMap<Chain>>(store()).remove(&viewport)).unwrap_or_default();
    let modal = ctx.memory(|m| m.top_modal_layer());
    let mut focus = ctx.memory(|m| m.focused());
    let given = give_back(ctx, &mut chain, modal, focus);
    focus = ctx.memory(|m| m.focused());
    // A widget under a modal layer gives up the keyboard now: egui sees to it a pass later, and
    // what is typed meanwhile would go under the dialog.
    if let Some(id) = focus
        && layer_of(ctx, id).is_some_and(|l| !ctx.memory(|m| m.allows_interaction(l)))
    {
        ctx.memory_mut(|m| m.surrender_focus(id));
        focus = None;
    }
    activate(ctx, &mut chain, focus);
    // A press given to a layer that nothing asked it of, and that wasn't drawn in its pass, went
    // nowhere: egui still showed the layer at the gate, but it had closed in the pass before (a
    // window, a dialog, the recurrence editor, with Escape a pass earlier). It is pressed again
    // now, once, as a press with no widget's keyboard in it (egui let go of that one then).
    let again: Vec<Key> = std::mem::take(&mut chain.sent)
        .into_iter()
        .filter(|(_, l, heard)| !heard && !ctx.memory(|m| m.areas().is_visible(l)))
        .map(|(key, _, _)| key)
        .collect();
    // Enter pressed in the pass the keyboard went back to a widget waits a pass: egui tells a
    // widget it let go of the keyboard (a field of one line does on Enter, and its form then
    // saves) only when it had it in the pass before.
    let later = std::mem::take(&mut chain.later);
    if given {
        ctx.input_mut(|i| {
            let before = i.events.len();
            i.events.retain(|e| !(alone(e, Key::Enter) && matches!(e, Event::Key { repeat: false, .. })));
            if i.events.len() < before {
                chain.later.push(Key::Enter);
            }
        });
    }
    ctx.input_mut(|i| {
        i.events.extend(again.iter().chain(&later).map(|key| Event::Key {
            key: *key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }))
    });
    let (escape, enter) = ctx.input(|i| {
        let new = |key| i.events.iter().any(|e| alone(e, key) && matches!(e, Event::Key { repeat: false, .. }));
        (new(Key::Escape), new(Key::Enter))
    });
    let list = open_list(ctx, focus).filter(|l| ctx.memory(|m| m.is_above_modal_layer(*l)));
    if chain.held.is_some_and(|(key, _)| !ctx.input(|i| i.key_down(key))) {
        chain.held = None;
    }
    for (key, pressed) in [(Key::Escape, escape), (Key::Enter, enter)] {
        if pressed {
            chain.held = Some((key, focus.filter(|_| list.is_none())));
        }
    }
    // A field that keeps the keyboard while its list is open (recipient suggestions) keeps it
    // while the Escape that closed the list is held: egui takes it away on the repeats.
    if escape && list.is_some() && focus.is_some() {
        chain.back = focus;
    }
    let owner = |key: Key| -> Option<Owner> {
        if blocked || list.is_some() {
            return None;
        }
        if let Some(top) = modal {
            // (What has the keyboard now is in it, or over it: under it, it was given up above.)
            return Some(Owner { layer: top, field: focus });
        }
        if key == Key::Escape && ctx.memory(|m| m.areas().is_visible(&crate::sync_panel::layer())) {
            return Some(Owner { layer: crate::sync_panel::layer(), field: None });
        }
        if let Some(id) = focus
            && let Some(layer) = layer_of(ctx, id)
        {
            return Some(Owner { layer: window_of(ctx, layer).unwrap_or(layer), field: Some(id) });
        }
        Some(Owner { layer: chain.active.first().copied().unwrap_or(main_layer()), field: None })
    };
    chain.pass = ctx.cumulative_pass_nr();
    let fresh = |key: Key, o: Option<Owner>| if again.contains(&key) { o.map(|o| Owner { field: None, ..o }) } else { o };
    chain.escape = fresh(Key::Escape, if escape { owner(Key::Escape) } else { None });
    chain.list_escape = escape && list.is_some() && !blocked;
    chain.enter = fresh(Key::Enter, if enter { owner(Key::Enter) } else { None });
    chain.sent = [(Key::Escape, chain.escape), (Key::Enter, chain.enter)]
        .into_iter()
        .filter_map(|(key, o)| o.map(|o| (key, o.layer, false)))
        .filter(|(key, layer, _)| *layer != main_layer() && !again.contains(key))
        .collect();
    // Enter's repeats type on in the text field the key went down in, while it has the keyboard;
    // not when it went down in a list, which took the press (a suggested recipient).
    let typing_on = !blocked && matches!(chain.held, Some((Key::Enter, Some(id))) if focus == Some(id) && types(ctx, id));
    // A new press stays for an open list (egui closes it on Escape), and Enter for the widget with
    // the keyboard (a text field types it, egui presses a button with it). Escape stays too when
    // it is the main window's, where nothing is over what egui does with it: it lets go of text
    // picked out in a label.
    let keep_escape = !blocked && (list.is_some() || chain.escape.is_some_and(|o| o.layer == main_layer() && o.field.is_none()));
    let keep_enter = !blocked && (list.is_some() || chain.enter.is_some_and(|o| o.field.is_some()));
    ctx.input_mut(|i| {
        i.events.retain(|e| match e {
            Event::Key { key: Key::Escape, repeat, .. } if alone(e, Key::Escape) => !repeat && keep_escape,
            Event::Key { key: Key::Enter, repeat, .. } if alone(e, Key::Enter) => {
                if *repeat {
                    typing_on
                } else {
                    keep_enter
                }
            }
            _ => true,
        });
    });
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<ViewportIdMap<Chain>>(store()).insert(viewport, chain);
    });
}

/// [`gate`] for the main window, at the start of its pass. The windows of the system that are
/// gone are forgotten.
pub(crate) fn gate_main(app: &MailApp, ctx: &egui::Context) {
    let live = crate::windows::viewports(app);
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<ViewportIdMap<Chain>>(store()).retain(|viewport, _| *viewport == ViewportId::ROOT || live.contains(viewport));
    });
    gate(ctx, false);
}

/// What asks for a press wants: none with a widget that had the keyboard, any, or that widget's.
#[derive(Clone, Copy)]
enum Want {
    Alone,
    Any,
    Field(egui::Id),
}

/// Whether a new press of `key` this pass is `layer`'s, as `want` has it; `take` keeps it from what
/// asks later in the pass.
fn ask(ctx: &egui::Context, layer: LayerId, key: Key, want: Want, take: bool) -> bool {
    let (pass, viewport) = (ctx.cumulative_pass_nr(), ctx.viewport_id());
    // A sublayer asks as its window; anything on the background as the main window.
    let layer = ctx.memory(|m| m.areas().parent_layer(layer)).unwrap_or(layer);
    let layer = if layer.order == Order::Background { main_layer() } else { layer };
    ctx.data_mut(|d| {
        let Some(chain) = d.get_temp_mut_or_default::<ViewportIdMap<Chain>>(store()).get_mut(&viewport) else { return false };
        if let Want::Field(id) = want
            && !chain.keepers.contains(&id)
        {
            chain.keepers.push(id);
            if chain.keepers.len() > REMEMBERED {
                chain.keepers.remove(0);
            }
        }
        if chain.pass != pass {
            return false;
        }
        for sent in chain.sent.iter_mut().filter(|(k, l, _)| *k == key && *l == layer) {
            sent.2 = true;
        }
        let slot = if key == Key::Escape { &mut chain.escape } else { &mut chain.enter };
        let mine = slot.is_some_and(|o| {
            o.layer == layer
                && match want {
                    Want::Alone => o.field.is_none(),
                    Want::Any => true,
                    Want::Field(id) => o.field == Some(id),
                }
        });
        if mine && take {
            *slot = None;
        }
        mine
    })
}

/// Whether Escape was pressed for `layer`, with no widget of it keeping the keyboard: it closes
/// what `layer` is, or ends what it is doing.
pub(crate) fn escape(ctx: &egui::Context, layer: LayerId) -> bool {
    ask(ctx, layer, Key::Escape, Want::Alone, false)
}

/// [`escape`], taken: what else of `layer` asks later in the pass is told no. Ask only when it
/// will be acted on.
pub(crate) fn take_escape(ctx: &egui::Context, layer: LayerId) -> bool {
    ask(ctx, layer, Key::Escape, Want::Alone, true)
}

/// Whether Escape was pressed for `layer`, in one of its widgets or with nothing focused, taken:
/// a panel that closes on Escape whatever of it has the keyboard (Advanced Search).
pub(crate) fn take_escape_any(ctx: &egui::Context, layer: LayerId) -> bool {
    ask(ctx, layer, Key::Escape, Want::Any, true)
}

/// Whether Escape was pressed in the widget `id` of `layer`, which keeps the keyboard on Escape
/// (the search box), taken.
pub(crate) fn take_escape_in(ctx: &egui::Context, layer: LayerId, id: egui::Id) -> bool {
    ask(ctx, layer, Key::Escape, Want::Field(id), true)
}

/// Whether this pass's new Escape is the open list's (a drop-down, a menu), which egui closes on
/// it by itself: for a menu over that list, a submenu of it, to close on it instead.
pub(crate) fn list_escape(ctx: &egui::Context) -> bool {
    let (pass, viewport) = (ctx.cumulative_pass_nr(), ctx.viewport_id());
    ctx.data(|d| d.get_temp::<ViewportIdMap<Chain>>(store()).and_then(|map| map.get(&viewport).map(|c| c.pass == pass && c.list_escape)))
        .unwrap_or(false)
}

/// Whether Enter was pressed for `layer`, in one of its fields or with nothing focused. (A text
/// field of the layer reads Enter itself, and a field of one line gives up the keyboard on it.)
pub(crate) fn enter(ctx: &egui::Context, layer: LayerId) -> bool {
    ask(ctx, layer, Key::Enter, Want::Any, false)
}

/// [`enter`], taken.
pub(crate) fn take_enter(ctx: &egui::Context, layer: LayerId) -> bool {
    ask(ctx, layer, Key::Enter, Want::Any, true)
}

/// [`take_enter`], and the press is taken out of the input too, so that no widget of `layer`
/// gets it either: a question whose Enter means one answer whatever has the keyboard in it.
/// Ask before the layer is drawn.
pub(crate) fn take_enter_out(ctx: &egui::Context, layer: LayerId) -> bool {
    let mine = take_enter(ctx, layer);
    if mine {
        ctx.input_mut(|i| i.events.retain(|e| !alone(e, Key::Enter)));
    }
    mine
}

/// Whether Enter was pressed for `layer` with nothing in it focused: OK, Open.
pub(crate) fn enter_alone(ctx: &egui::Context, layer: LayerId) -> bool {
    ask(ctx, layer, Key::Enter, Want::Alone, false)
}

/// [`enter_alone`], taken.
pub(crate) fn take_enter_alone(ctx: &egui::Context, layer: LayerId) -> bool {
    ask(ctx, layer, Key::Enter, Want::Alone, true)
}

/// The keyboard went back to `to` as a list closed on Escape (a date field's calendar): while that
/// Escape is held, its repeats leave the keyboard there (#329).
pub(crate) fn give_back_on_escape(ctx: &egui::Context, to: egui::Id) {
    if !ctx.input(|i| i.key_down(Key::Escape)) {
        return;
    }
    let viewport = ctx.viewport_id();
    ctx.data_mut(|d| {
        if let Some(chain) = d.get_temp_mut_or_default::<ViewportIdMap<Chain>>(store()).get_mut(&viewport) {
            chain.back = Some(to);
        }
    });
}

/// The window that is active in the viewport being laid out: its own background in a window
/// of the system; in the main one, the main window or one of the windows inside it.
pub(crate) fn active(ctx: &egui::Context) -> LayerId {
    let viewport = ctx.viewport_id();
    ctx.data_mut(|d| d.get_temp_mut_or_default::<ViewportIdMap<Chain>>(store()).get(&viewport).and_then(|c| c.active.first().copied()))
        .unwrap_or(main_layer())
}

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
        // `=` is where `+` is on most keyboards: Mod with either zooms in.
        Key::Plus | Key::Equals => "Plus",
        Key::Minus => "Minus",
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

/// The commands that move the focus in the message list (#426): the list scrolls to the row they
/// move to, and with Shift they extend the selection.
pub const MOVES_IN_LIST: &[&str] = &["mail.next", "mail.previous", "mail.first", "mail.last", "mail.pageDown", "mail.pageUp"];

/// What a key gives the command it runs: with Shift, a move in the message list extends the
/// selection, and a page is the rows that fit in the list.
pub fn key_params(app: &MailApp, id: &str, m: Modifiers) -> serde_json::Value {
    let mut p = serde_json::Map::new();
    if m.shift && MOVES_IN_LIST.contains(&id) {
        p.insert("extend".into(), json!(true));
    }
    if matches!(id, "mail.pageDown" | "mail.pageUp") {
        p.insert("rows".into(), json!(app.list_page_rows.max(1)));
    }
    serde_json::Value::Object(p)
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

/// Drops the repeats of `keys` held down, so that what reads them later in the frame sees only the
/// press: a held key is one press (#391, #393).
pub(crate) fn drop_repeats(i: &mut egui::InputState, keys: &[Key]) {
    i.events.retain(|e| !matches!(e, egui::Event::Key { key, pressed: true, repeat: true, .. } if keys.contains(key)));
}

/// Whether the window takes command keys now: not while File (backstage), a dialog or an agent's
/// question is open (#503). The menu bar asks the same, so its items are greyed then.
pub fn takes_commands(app: &MailApp) -> bool {
    !app.ui.backstage && app.dialog.is_none() && app.asking.is_empty()
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
        // Escape closes File when it is on top: not when a dialog, a menu or a window is over
        // it, and not on the repeats of a held Escape (#393, #406).
        if app.ui.backstage && take_escape(ctx, main_layer()) {
            app.ui.backstage = false;
        }
        return;
    }
    let typing = ctx.egui_wants_keyboard_input();
    // Escape is the main window's only when nothing is over it (#406); it is taken only by what
    // acts on it, so that what else of the window reads it later still can. It ends a search, as
    // the ✕ on the bar over the list does (#115). (In the search box itself, the box does it.)
    if !typing
        && app.session.compose.is_none()
        && app.session.view.module == mailcraft_engine::Module::Mail
        && !app.session.view.search.trim().is_empty()
        && take_escape(ctx, main_layer())
    {
        app.search_text.clear();
        let _ = app.run("view.clearSearch", json!({}));
    }
    // Otherwise it is the registry's: it closes a message being written, from one of its fields
    // too (#365).
    if (!typing || app.session.compose.is_some())
        && let Some(id) = resolve(app, "Escape").and_then(|id| window_command(app, id))
        && take_escape(ctx, main_layer())
    {
        let _ = app.run(id, json!({}));
    }
    // The body of a message being written has keys of its own (Ctrl+B, U, Z…), which mean
    // something else everywhere else.
    let in_body = typing && crate::compose::editor::has_focus(ctx);
    let events = ctx.input(|i| i.events.clone());
    // An open list (a menu, a date field's calendar) keeps the keys that move in it, with Shift or
    // Ctrl too: the message list doesn't move under it (#329).
    let list = egui::Popup::is_any_open(ctx);
    for e in events {
        // A key that opened a dialog (Delete's question) leaves the frame's other keys to it (#503).
        if !takes_commands(app) {
            break;
        }
        let egui::Event::Key { key, pressed: true, modifiers, .. } = e else { continue };
        // Escape alone is handed out above (an open menu keeps it, to close).
        if key == Key::Escape && modifiers.is_none() {
            continue;
        }
        let moving = matches!(
            key,
            Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown | Key::PageUp | Key::PageDown | Key::Home | Key::End | Key::Space
        );
        if list && moving {
            continue;
        }
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
            if !(function || chord && !editing) {
                continue;
            }
        }
        // A `+` typed with Shift is the same key as one without.
        let modifiers = if name == "Plus" { Modifiers { shift: false, ..modifiers } } else { modifiers };
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
            let params = key_params(app, id, modifiers);
            let _ = app.run(id, params);
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
