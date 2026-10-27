// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The menu of a text the owner types in (#557): Cut, Copy, Paste and Select All on a right
//! click, in the body of a message or a signature ([`body`]) and in egui's text fields ([`field`],
//! [`secret`], [`read_only`]).
//!
//! - An item does what its key does: it becomes the input that key gives (`Event::Cut`,
//!   `Event::Copy`, what Ctrl+V gives, Ctrl+A), handed to the text at the start of its window's
//!   next pass with the keyboard and what was selected given back ([`deliver`]). No command.
//! - Cut and Copy are greyed while nothing is selected; a password offers Paste alone, a text that
//!   is only read Copy and Select All; Paste is greyed with no host clipboard (the browser).
//! - Using the menu isn't leaving the text ([`left`]).
//! - A choice made by input the control channel injected (`MailApp::driven`) cuts, copies and
//!   pastes nothing.

use egui::containers::menu::MenuState;
use egui::text::{CCursor, CCursorRange};
use egui::{Context, Event, Id, Key, LayerId, Modifiers, Order, PointerButton, Popup, Pos2, Response, TextEdit, Ui, ViewportId, vec2};

use crate::MailApp;

/// What a text is, for its menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Edited: everything.
    Text,
    /// A password: Paste alone.
    Secret,
    /// Only read: Copy and Select All.
    ReadOnly,
}

/// An item of a text's menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Cut,
    Copy,
    Paste,
    SelectAll,
}

/// What a text takes from the clipboard besides text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Takes {
    /// Text alone: egui's fields.
    Text,
    /// HTML and pictures: a signature, and the body of a message (#547).
    Pictures,
}

/// What was chosen in a text's menu, until [`carry_out`]: the window of the system it was chosen
/// in, the text, the item, and what the text takes.
#[derive(Clone, Copy, Debug)]
struct Asked {
    viewport: ViewportId,
    target: Id,
    item: Item,
    takes: Takes,
}

/// The input a text is to be given at the start of the next pass of its window ([`deliver`]);
/// `pass`: the main window's when it was carried out.
#[derive(Clone, Debug)]
struct Pending {
    viewport: ViewportId,
    target: Id,
    takes: Takes,
    events: Vec<Event>,
    /// What a body's Paste found on the clipboard besides its text: it goes with the paste, and
    /// with it when it is dropped.
    captured: Option<Captured>,
    pass: u64,
}

/// The menu of a text that is in a drop-down or a menu: whether it is open, where it opened, and
/// the pass its text was last drawn in. (Any other text's menu is the popup egui keeps open.)
#[derive(Clone, Copy, Debug, Default)]
struct Nested {
    open: bool,
    at: Pos2,
    pass: u64,
}

/// The text whose menu is open in a window, as the window's pass before left it.
#[derive(Clone, Copy, Debug)]
struct OpenMenu {
    target: Id,
    /// The text had the keyboard.
    focused: bool,
}

/// The HTML a body's Paste found on the clipboard beside the text when it was chosen: the body
/// pastes this rather than what is on the clipboard by the time it reads it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Captured {
    pub html: Option<String>,
}

/// How many passes of the main window input waits for a window that isn't drawn (it closed).
const PENDING_PASSES: u64 = 30;

fn asked() -> Id {
    Id::unique("mailcraft-text-menu")
}

fn pending() -> Id {
    asked().with("pending")
}

/// Where the window `viewport` keeps the text whose menu is open in it.
fn open_menu(viewport: ViewportId) -> Id {
    asked().with(("open", viewport))
}

/// Where the main window keeps, for each pass, how a click elsewhere takes the keyboard when no
/// text's menu is pointed at (the windows of the system lay out inside its pass).
fn surrender() -> Id {
    asked().with("surrender")
}

/// The layer a text's menu is drawn in (egui's popup of the text's response, either way).
fn menu_layer(id: Id) -> LayerId {
    LayerId::new(Order::Foreground, id.with("popup"))
}

/// Whether the pointer is on the menu of the text `id`, by what was last drawn.
fn on_menu(ctx: &Context, id: Id) -> bool {
    ctx.input(|i| i.pointer.interact_pos()).and_then(|pos| ctx.layer_id_at(pos)) == Some(menu_layer(id))
}

/// Whether this pass's press is on the menu of the text `id`: a press that is not ends what the
/// text was doing, as it does with no menu open (the calendar's typed entry is made).
pub(crate) fn press_on_menu(ctx: &Context, id: Id) -> bool {
    ctx.input(|i| i.pointer.any_pressed()) && on_menu(ctx, id)
}

fn captured(id: Id) -> Id {
    id.with("text-menu-paste")
}

/// Where [`carry_out`] says whether there is a clipboard to paste from.
fn pasting() -> Id {
    asked().with("paste")
}

/// Where the menu of the text `id` keeps whether it is open, when it is a submenu ([`Nested`]).
fn shown(id: Id) -> Id {
    id.with("text-menu")
}

/// Where what was last selected in the text field `id` while it had the keyboard is kept.
fn kept(id: Id) -> Id {
    id.with("text-menu-kept")
}

/// An item of a text's menu, as the reading pane's are drawn: its name and its key.
pub(crate) fn menu_item(ui: &mut Ui, enabled: bool, label: &str, key: &str) -> Response {
    let button = egui::Button::new(tl!(label)).min_size(vec2(190.0, 0.0));
    let button = if key.is_empty() { button } else { button.shortcut_text(crate::widgets::key_text(key)) };
    ui.add_enabled(enabled, button)
}

/// The items for a text of `kind`, `selected` whether anything is selected in it, `paste`
/// whether there is a clipboard to paste from. What was chosen; the menu shuts then.
fn items(ui: &mut Ui, kind: Kind, selected: bool, paste: bool) -> (Option<Item>, bool) {
    let (mut chosen, mut keyboard) = (None, false);
    let edits = kind == Kind::Text;
    let shown = [
        (edits, selected, "Cut", "Mod+X", Item::Cut),
        (kind != Kind::Secret, selected, "Copy", "Mod+C", Item::Copy),
        (kind != Kind::ReadOnly, paste, "Paste", "Mod+V", Item::Paste),
        (kind != Kind::Secret, true, "Select All", "Mod+A", Item::SelectAll),
    ];
    for (offered, enabled, label, key, item) in shown {
        if !offered {
            continue;
        }
        let resp = menu_item(ui, enabled, label, key);
        keyboard |= resp.has_focus();
        if resp.clicked() {
            chosen = Some(item);
        }
    }
    if chosen.is_some() {
        ui.close();
    }
    (chosen, keyboard)
}

/// The menu on the text `resp`.
///
/// It is the one popup egui keeps open in a window, which the keys know of (`crate::keys`: Escape
/// closes it, and nothing under it). A text in a drop-down or a menu, which is that popup already,
/// has its menu as a submenu of it instead: open by its own note ([`Nested`]), and counted by the
/// drop-down as one of its own (`MenuState`), so that neither closes the other.
fn menu(resp: &Response, kind: Kind, selected: bool, takes: Takes) {
    let ctx = &resp.ctx;
    let id = Popup::default_response_id(resp);
    let pass = ctx.cumulative_pass_nr();
    let mut nested: Option<Nested> = ctx.data(|d| d.get_temp(shown(resp.id)));
    // A submenu whose text wasn't drawn in the pass before went with it (a drop-down that closed).
    if nested.is_some_and(|n| !n.open || n.pass.saturating_add(1) < pass) {
        nested = None;
    }
    if resp.secondary_clicked() {
        let within = resp.layer_id.order == Order::Foreground && Popup::is_any_open(ctx) && !Popup::is_id_open(ctx, id);
        let at = ctx.input(|i| i.pointer.interact_pos()).unwrap_or(resp.rect.left_bottom());
        nested = within.then_some(Nested { open: true, at, pass });
    } else if resp.clicked() {
        nested = None;
    }
    let paste = ctx.data(|d| d.get_temp::<bool>(pasting())).unwrap_or(false);
    let (mut chosen, mut keyboard) = (None, false);
    let open = match nested.as_mut() {
        Some(n) => {
            n.pass = pass;
            let at = n.at;
            // The Escape that is the open list's (`crate::keys`), the drop-down's, closes this menu
            // only: taken out of the input, it doesn't close the drop-down as well.
            if crate::keys::list_escape(ctx) && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                n.open = false;
            }
            Popup::menu(resp).open_bool(&mut n.open).at_position(at).show(|ui| (chosen, keyboard) = items(ui, kind, selected, paste));
            if n.open {
                MenuState::from_id(ctx, resp.layer_id.id, |state| state.open_item = Some(id));
            }
            n.open
        }
        None => {
            Popup::context_menu(resp).show(|ui| (chosen, keyboard) = items(ui, kind, selected, paste));
            Popup::is_id_open(ctx, id)
        }
    };
    if open {
        let (viewport, focused) = (ctx.viewport_id(), resp.has_focus() || keyboard);
        ctx.data_mut(|d| d.insert_temp(open_menu(viewport), OpenMenu { target: resp.id, focused }));
    }
    // An item of it has the keyboard (a screen reader moved it there): the text is still being
    // edited, not left ([`left`]).
    if open && keyboard {
        ctx.data_mut(|d| d.insert_temp(keyboard_in(resp.id), pass));
    }
    match nested {
        Some(n) => ctx.data_mut(|d| {
            d.insert_temp(shown(resp.id), n);
        }),
        None => ctx.data_mut(|d| d.remove::<Nested>(shown(resp.id))),
    }
    if let Some(item) = chosen {
        let viewport = ctx.viewport_id();
        ctx.data_mut(|d| d.insert_temp(asked(), Asked { viewport, target: resp.id, item, takes }));
    }
}

/// Whether the text field `resp` was left in this pass, as its `lost_focus` says, but not for its
/// own menu: true once, when it loses the keyboard to anything else. Left while its menu has the
/// keyboard (an item of it, or a choice still to be given to it), it is left in the first pass
/// after the menu lets go, unless it has the keyboard back by then. For a text that does
/// something when it is left (a clock tidied, names made contacts, the calendar's entry made).
pub(crate) fn left(resp: &Response) -> bool {
    let (ctx, wait) = (&resp.ctx, resp.id.with("text-menu-left"));
    if resp.has_focus() {
        ctx.data_mut(|d| d.remove::<bool>(wait));
        return false;
    }
    let waiting = ctx.data(|d| d.get_temp::<bool>(wait)).unwrap_or(false);
    // egui's `lost_focus` holds for two passes: the second is the same leaving.
    let (pass, fired) = (ctx.cumulative_pass_nr(), resp.id.with("text-menu-left-at"));
    let again = ctx.data(|d| d.get_temp::<u64>(fired)).is_some_and(|at| at.saturating_add(1) >= pass);
    let lost = (resp.lost_focus() && !again) || waiting;
    if busy(ctx, resp.id) {
        if lost && !waiting {
            ctx.data_mut(|d| d.insert_temp(wait, true));
        }
        return false;
    }
    if waiting {
        ctx.data_mut(|d| d.remove::<bool>(wait));
    }
    if lost {
        ctx.data_mut(|d| d.insert_temp(fired, pass));
    }
    lost
}

/// Whether the text `id` is without the keyboard only for its own menu: an item of the menu has
/// it, or what was chosen in it is still to be given to the text.
fn busy(ctx: &Context, id: Id) -> bool {
    let pass = ctx.cumulative_pass_nr();
    let asked = ctx.data(|d| d.get_temp::<Asked>(asked())).is_some_and(|a| a.target == id);
    let pending = ctx.data(|d| d.get_temp::<Pending>(pending())).is_some_and(|p| p.target == id);
    let keyboard = ctx.data(|d| d.get_temp::<u64>(keyboard_in(id))).is_some_and(|at| at.saturating_add(1) >= pass);
    asked || pending || keyboard
}

/// Where the text `id` notes the pass an item of its menu last had the keyboard in.
fn keyboard_in(id: Id) -> Id {
    id.with("text-menu-keyboard")
}

/// A text field egui draws, `resp` (what `ui.add(TextEdit…)` answered), with the menu.
pub(crate) fn field(resp: Response) -> Response {
    attach(resp, Kind::Text)
}

/// A password field, with the menu: Paste alone.
pub(crate) fn secret(resp: Response) -> Response {
    attach(resp, Kind::Secret)
}

/// A text field whose text is only read, with the menu: Copy and Select All.
pub(crate) fn read_only(resp: Response) -> Response {
    attach(resp, Kind::ReadOnly)
}

/// Whether `at` is on the selection `range`.
fn on(range: CCursorRange, at: CCursor) -> bool {
    let r = range.as_sorted_char_range();
    !range.is_empty() && r.start <= at.index && at.index <= r.end
}

fn attach(resp: Response, kind: Kind) -> Response {
    let ctx = resp.ctx.clone();
    let mut state = TextEdit::load_state(&ctx, resp.id);
    let was = ctx.data(|d| d.get_temp::<CCursorRange>(kept(resp.id)));
    // egui puts the caret where any button goes down: a right click on what is selected gives it
    // back.
    let pressed = resp.hovered() && !resp.gained_focus() && ctx.input(|i| i.pointer.button_pressed(PointerButton::Secondary));
    if pressed
        && let (Some(was), Some(state)) = (was, state.as_mut())
        && state.cursor.char_range().is_some_and(|now| on(was, now.primary))
    {
        state.cursor.set_char_range(Some(was));
        state.clone().store(&ctx, resp.id);
    }
    // egui lets go of a field's selection as soon as the field hasn't the keyboard, which a click
    // on its menu takes: what was selected while it had it is kept for the menu, and given back
    // with the keyboard when an item is chosen ([`deliver`]).
    let now = state.and_then(|s| s.cursor.char_range());
    let range = if resp.has_focus() { now } else { was };
    if resp.has_focus()
        && let Some(now) = now
    {
        ctx.data_mut(|d| d.insert_temp(kept(resp.id), now));
    }
    menu(&resp, kind, range.is_some_and(|r| !r.is_empty()), Takes::Text);
    resp
}

/// The body of a message being written, or a signature being edited, `resp`, with the menu;
/// `selected`: whether anything is selected in it; `takes`: what a paste brings into it. (Where a
/// right click puts the caret is the body's own.)
pub(crate) fn body(resp: &Response, selected: bool, takes: Takes) {
    menu(resp, Kind::Text, selected, takes);
}

/// What was chosen this pass becomes its input, for [`deliver`]. The extra pass asked for runs in
/// the same frame with no input of its own, so nothing typed or clicked after the choice comes
/// before it. Once a pass of the main window, after every window is drawn.
pub(crate) fn carry_out(app: &mut MailApp, ctx: &Context) {
    ctx.data_mut(|d| d.insert_temp(pasting(), app.services.clipboard.is_some()));
    let pass = ctx.cumulative_pass_nr();
    // Input for a window that has gone since.
    if ctx.data(|d| d.get_temp::<Pending>(pending())).is_some_and(|p| p.pass.saturating_add(PENDING_PASSES) < pass) {
        ctx.data_mut(|d| d.remove::<Pending>(pending()));
    }
    let Some(asked) = ctx.data(|d| d.get_temp::<Asked>(asked())) else { return };
    ctx.data_mut(|d| d.remove::<Asked>(self::asked()));
    // What the control channel's input chose gives the text the keyboard back, and no more.
    let refused = app.driven && asked.item != Item::SelectAll;
    let events = match asked.item {
        _ if refused => (Vec::new(), None),
        Item::Cut => (vec![Event::Cut], None),
        Item::Copy => (vec![Event::Copy], None),
        Item::SelectAll => (
            [true, false]
                .into_iter()
                .map(|pressed| Event::Key { key: Key::A, physical_key: Some(Key::A), pressed, repeat: false, modifiers: Modifiers::COMMAND })
                .collect(),
            None,
        ),
        Item::Paste => paste(app, asked.takes),
    };
    let (events, captured) = events;
    let pending = Pending { viewport: asked.viewport, target: asked.target, takes: asked.takes, events, captured, pass };
    ctx.data_mut(|d| d.insert_temp(self::pending(), pending));
    ctx.request_discard("a choice in a text's menu, given to the text before anything else");
    ctx.request_repaint_of(asked.viewport);
}

/// What Paste gives a text that takes `takes`, the clipboard read once, now: what Ctrl+V gives,
/// and for a body the HTML beside the text ([`Captured`]).
fn paste(app: &MailApp, takes: Takes) -> (Vec<Event>, Option<Captured>) {
    let Some(board) = app.services.clipboard.as_ref() else { return (Vec::new(), None) };
    // What the system's backend hands egui for Ctrl+V (egui-winit), from the same reads: the text
    // when there is some, line ends as `\n`; nothing for an empty one; the picture only when there
    // is no text at all. The body then reads the HTML beside the text, as for Ctrl+V.
    match board.text() {
        Some(text) => {
            let text = text.replace("\r\n", "\n");
            if text.is_empty() {
                return (Vec::new(), None);
            }
            let captured = (takes != Takes::Text).then(|| Captured { html: board.html().filter(|html| !html.trim().is_empty()) });
            (vec![Event::Paste(text)], captured)
        }
        // (Only a signature or a message's body takes a picture: anywhere else egui's
        // `PasteImage` changes nothing.)
        None if takes == Takes::Pictures => (board.image().and_then(|png| picture(&png)).into_iter().collect(), None),
        None => (Vec::new(), None),
    }
}

/// A picture on the clipboard, PNG, as egui's backend hands one over (`Event::PasteImage`); none
/// when it is larger than a picture from the clipboard is taken.
fn picture(png: &[u8]) -> Option<Event> {
    let reader = image::ImageReader::with_format(std::io::Cursor::new(png), image::ImageFormat::Png);
    let (width, height) = reader.into_dimensions().ok()?;
    let side = |n: u32| usize::try_from(n).is_ok_and(|n| n <= crate::MAX_PICTURE_SIDE);
    if !side(width) || !side(height) {
        return None;
    }
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8();
    let size = [usize::try_from(image.width()).ok()?, usize::try_from(image.height()).ok()?];
    Some(Event::PasteImage(std::sync::Arc::new(egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()))))
}

/// The field `id` takes digits alone (a port): what is pasted into it, by its menu or Ctrl+V, is
/// the digits of it, and nothing when it has none, so that what is selected stays. Before the
/// field is laid out, while it has the keyboard.
pub(crate) fn digits_only(ctx: &Context, id: Id) {
    if !ctx.memory(|m| m.has_focus(id)) {
        return;
    }
    ctx.input_mut(|i| {
        i.events.retain_mut(|e| match e {
            Event::Paste(text) => {
                text.retain(|c| c.is_ascii_digit());
                !text.is_empty()
            }
            _ => true,
        });
    });
}

/// What the menu's Paste found on the clipboard for the body `id`, when its paste is being given
/// to it in this pass (the body pastes that, not what is on the clipboard by now). Nothing of it
/// is left after the pass ([`finish`]).
pub(crate) fn taken_paste(ctx: &Context, id: Id) -> Option<Captured> {
    let found = ctx.data(|d| d.get_temp::<Captured>(captured(id)));
    if found.is_some() {
        ctx.data_mut(|d| d.remove::<Captured>(captured(id)));
    }
    found
}

/// At the start of every pass of each window, before anything reads the input:
/// - while the pointer is on a text's menu, a click takes the keyboard from nobody
///   (`SurrenderFocusOn::Never`), so the text keeps it through a click on its menu, and a click
///   anywhere else leaves it as it does with no menu open; when egui lets go of it as the pass
///   begins (on the Escape that closes the menu), the text has it back before anything sees it gone;
/// - what was chosen in its menu is given to it: the keyboard, what was selected (all of a box of
///   a number not yet typed into), then the input, before any other.
pub(crate) fn deliver(ctx: &Context) -> Scope {
    let viewport = ctx.viewport_id();
    let open = ctx.data(|d| d.get_temp::<OpenMenu>(open_menu(viewport)));
    let over = open.is_some_and(|m| on_menu(ctx, m.target));
    // For this window's pass only, [`finish`] puts back what was: nothing of it is left for another
    // window's pass, or for a window that closed.
    let was = ctx.options(|o| o.input_options.surrender_focus_on);
    let base = if viewport == ViewportId::ROOT {
        ctx.data_mut(|d| d.insert_temp(surrender(), was));
        was
    } else {
        ctx.data(|d| d.get_temp::<egui::SurrenderFocusOn>(surrender())).unwrap_or(was)
    };
    ctx.options_mut(|o| o.input_options.surrender_focus_on = if over { egui::SurrenderFocusOn::Never } else { base });
    let mut scope = Scope { was, captured: None };
    if let Some(menu) = open {
        ctx.data_mut(|d| d.remove::<OpenMenu>(open_menu(viewport)));
        // (Only when nothing has it: the keyboard moved to another widget, a menu item by a screen
        // reader say, stays there.)
        if menu.focused && ctx.memory(|m| m.focused()).is_none() && !ctx.input(|i| i.pointer.any_pressed()) {
            ctx.memory_mut(|m| m.request_focus(menu.target));
        }
    }
    let Some(pending) = ctx.data(|d| d.get_temp::<Pending>(pending())).filter(|p| p.viewport == viewport) else { return scope };
    ctx.data_mut(|d| d.remove::<Pending>(self::pending()));
    // egui had no pass to spare for it in the frame it was chosen in, and in this one something
    // came first: a press, which may have moved the keyboard already, or the text is gone. It is
    // dropped whole, what its Paste read with it, rather than given to what has the keyboard now.
    if ctx.input(|i| i.pointer.any_pressed()) || ctx.read_response(pending.target).is_none() {
        return scope;
    }
    if let Some(found) = pending.captured {
        ctx.data_mut(|d| d.insert_temp(captured(pending.target), found));
        scope.captured = Some(pending.target);
    }
    let focused = ctx.memory(|m| m.has_focus(pending.target));
    ctx.memory_mut(|m| m.request_focus(pending.target));
    let kept = ctx.data(|d| d.get_temp::<CCursorRange>(kept(pending.target)));
    let range = match kept {
        Some(range) => Some(range),
        // A box of a number (egui's `DragValue`) becomes a text field as it gets the keyboard:
        // the whole number is selected, as when it is clicked into.
        None if !focused && pending.takes == Takes::Text => Some(CCursorRange::two(CCursor::new(0), CCursor::new(usize::MAX))),
        None => None,
    };
    if let Some(range) = range {
        let mut state = TextEdit::load_state(ctx, pending.target).unwrap_or_default();
        state.cursor.set_char_range(Some(range));
        state.store(ctx, pending.target);
    }
    ctx.input_mut(|i| {
        let newer = std::mem::take(&mut i.events);
        i.events = pending.events;
        i.events.extend(newer);
    });
    scope
}

/// How a click elsewhere took the keyboard as a window's pass began, for [`finish`] to put back;
/// and what of a Paste is to go with the pass.
#[must_use]
pub(crate) struct Scope {
    was: egui::SurrenderFocusOn,
    /// The body given a Paste in this pass, whose clipboard's content is not to outlive it.
    captured: Option<Id>,
}

/// The end of a window's pass that began with [`deliver`].
pub(crate) fn finish(ctx: &Context, scope: Scope) {
    ctx.options_mut(|o| o.input_options.surrender_focus_on = scope.was);
    if let Some(id) = scope.captured {
        ctx.data_mut(|d| d.remove::<Captured>(captured(id)));
    }
}
