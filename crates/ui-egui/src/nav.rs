// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Getting around: the switch between the modules (Mail, Calendar, People, Tasks), as a bar of
//! icons down the left edge or as a row at the foot of the pane on the left (#1174), and the
//! folder pane beside it in Mail.

use std::collections::{HashMap, HashSet};

use egui::{Align2, Color32, CornerRadius, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use mailcraft_engine::Module;
use mailcraft_engine::cmd::folder::sort_as_listed;
use mailcraft_model::{Folder, FolderKind, GmailCategory, Id, Mailbox};
use serde_json::json;

use crate::favorites_sync::Note;
use crate::theme::{Tokens, bold, paint_clipped, regular};
use crate::{MailApp, icons};

/// The module bar's width, and the height each module takes in it.
const RAIL_W: f32 = 48.0;
const RAIL_PITCH: f32 = 48.0;

/// A folder's row.
const ROW_H: f32 = 24.0;
/// Folder names and counts are 9 pt, the Favorites and account headings 11 pt.
const NAME_SIZE: f32 = 12.0;
const HEADING_SIZE: f32 = 14.667;
/// Where a top-level name starts, and how much further in each level under it starts. This and
/// the distances below are from the pane's edges, the line down its left edge included.
const NAME_LEFT: f32 = 26.0;
const INDENT: f32 = 9.0;
/// The deepest level that still moves further in (deeper folders line up with it).
const MAX_INDENT_DEPTH: usize = 16;
/// Where a row's or a heading's chevron is centred.
const CHEVRON_X: f32 = 15.0;
/// Half the width of a heading's chevron, and of a row's.
const HEADING_CHEVRON: f32 = 4.75;
const ROW_CHEVRON: f32 = 4.25;
/// Counts end this far from the pane's right edge.
const COUNT_RIGHT: f32 = 16.0;
/// The highlight of a row (selected, or under the pointer) stops this short of the pane's edges.
const HIGHLIGHT_LEFT: f32 = 6.0;
const HIGHLIGHT_RIGHT: f32 = 4.0;
/// Inset each row's background so adjacent highlights have a gap without shrinking the hit area.
const ROW_HIGHLIGHT_INSET: f32 = 1.0;
/// A line of text is centred by its line box; the eye centres it by its lower-case letters, which
/// sit this much below the middle of the box.
/// How round the pane's corner is where it meets the module bar and the ribbon.
const SHEET_CORNER: u8 = 8;
/// How wide the strip is that stays when the pane is hidden.
const STRIP_W: f32 = 30.0;

/// The id the Favorites heading is folded under in `UiState::collapsed_folders`, beside the
/// folders' ids and the accounts' (an account's heading folds under the account's own id).
/// Accounts and folders are numbered from 1 in one sequence, so none of the three collide.
const FAVORITES: Id = 0;
/// The dot of an account's colour after its name in its heading (#893), and the room it takes there.
const ACCOUNT_DOT_RADIUS: f32 = 4.5;
const ACCOUNT_DOT_ROOM: f32 = 16.0;

/// The module bar. It runs from under the title bar down to the status bar, beside the tabs and
/// the ribbon as well as the panes, so the window lays it out before the ribbon.
pub fn module_bar(app: &mut MailApp, ui: &mut Ui) {
    // At the foot of the pane on the left instead, where older mail programs had it (#1174).
    if foot_in_use(&app.session) {
        return;
    }
    let t = Tokens::get(ui.ctx());
    egui::Panel::left("modules").exact_size(RAIL_W).resizable(false).show_separator_line(false).frame(egui::Frame::NONE.fill(t.nav)).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        for (m, icon, id) in module_buttons() {
            let (r, resp) = ui.allocate_exact_size(vec2(RAIL_W, RAIL_PITCH), Sense::click());
            let active = app.session.view.module == m;
            if active {
                // The module in view: a bar at the window's edge, and its icon in the accent.
                let bar = Rect::from_min_size(pos2(r.min.x + 6.0, r.center().y - 16.0), vec2(2.0, 32.0));
                ui.painter().rect_filled(bar, 1.0, t.accent);
            } else if resp.hovered() {
                ui.painter().rect_filled(r.shrink(6.0), 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
            }
            // One colour: the icons of the bar are line drawings, lit when theirs is the module in view.
            let ink = if active { t.accent_text } else { t.icon };
            icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(24.0, 24.0)), icon, ink, ink);
            let sc = crate::widgets::shortcut_text(app, id);
            if resp.on_hover_text(format!("{} ({sc})", tl!(m.label()))).clicked() {
                let _ = app.run(id, json!({}));
            }
        }
        // What a .pst file's notes and journal entries will be read in: kept, with no view yet.
        for (icon, name) in [("notes", tl!("Notes")), ("journal", tl!("Journal"))] {
            let (r, resp) = ui.allocate_exact_size(vec2(RAIL_W, RAIL_PITCH), Sense::hover());
            let ink = t.icon.linear_multiply(0.45);
            icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(24.0, 24.0)), icon, ink, ink);
            resp.on_hover_text(format!("{name}: {}", tl!("Pending development")));
        }
    });
}

/// How tall the row of modules is at the foot of the pane on the left (#1174), and how large a
/// module's name is written there.
const FOOT_H: f32 = 34.0;
const FOOT_NAME: f32 = 12.0;
/// What a name needs beside its icon in the row: the icon, the gap to it, and a little air.
const FOOT_NAME_ROOM: f32 = 22.0;

/// The icon and command id of each module, in the order both the bar and the foot show them.
fn module_buttons() -> [(Module, &'static str, &'static str); 4] {
    [
        (Module::Mail, "mail", "view.mail"),
        (Module::Calendar, "calendar", "view.calendar"),
        (Module::People, "people", "view.people"),
        (Module::Tasks, "tasks", "view.tasks"),
    ]
}

/// Whether the row of modules sits at the foot of the pane on the left (#1174): the owner asked
/// for it, and the module in view has a pane for it. Mail's folder pane and the calendar's side
/// pane can be hidden, and what is left of Mail's is a strip too narrow for four modules; People
/// and Tasks always have theirs. Without a pane the bar down the window's edge stays, so the
/// switch is never out of reach.
pub fn foot_in_use(s: &mailcraft_engine::Session) -> bool {
    s.view.nav_foot && (s.view.folder_pane || matches!(s.view.module, Module::People | Module::Tasks))
}

/// Whether a module's name is written beside its icon in the row (#1174): all four or none, so
/// that the row doesn't read half named, and only where the pane is wide enough for the longest
/// of them. A narrow pane leaves the icons alone, which is still the whole switch: what the row
/// is for is the column the bar took down the window, not the names.
pub(crate) fn names_fit(each: f32, widest: f32) -> bool {
    each >= widest + FOOT_NAME_ROOM
}

/// The switch between the modules at the foot of the pane on the left (#1174), where older mail
/// programs had it: an icon for each, side by side, with their names where the pane is wide
/// enough ([`names_fit`]), and the module in view in the accent with a line under it. Nothing
/// while the bar down the window's edge is the one in use, so every pane may call it.
pub fn module_foot(app: &mut MailApp, ui: &mut Ui) {
    if !foot_in_use(&app.session) {
        return;
    }
    let t = Tokens::get(ui.ctx());
    egui::Panel::bottom("modules_foot").exact_size(FOOT_H).resizable(false).show_separator_line(false).frame(egui::Frame::NONE.fill(t.nav)).show(
        ui,
        |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let room = ui.max_rect();
            let each = (room.width() / 4.0).max(1.0);
            // The names show for all four or for none, so the row doesn't read half named as the
            // pane narrows: the longest of them is what has to fit.
            let widest = module_buttons()
                .into_iter()
                .map(|(m, _, _)| crate::theme::text_width(ui.painter(), tl!(m.label()), &regular(FOOT_NAME), 0.0, t.text))
                .fold(0.0f32, f32::max);
            let named = names_fit(each, widest);
            for (i, (m, icon, id)) in module_buttons().into_iter().enumerate() {
                let r = Rect::from_min_size(pos2(room.min.x + each * i as f32, room.min.y), vec2(each, room.height()));
                let resp = ui.interact(r, ui.scope_id().with(("module_foot", icon)), Sense::click());
                let active = app.session.view.module == m;
                if active {
                    let line = Rect::from_min_size(pos2(r.min.x + 4.0, r.max.y - 2.0), vec2((r.width() - 8.0).max(0.0), 2.0));
                    ui.painter().rect_filled(line, 1.0, t.accent);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r.shrink(3.0), 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
                }
                // Reached with Tab or the arrow keys (Enter or Space switches): where the keyboard is.
                if resp.has_focus() {
                    ui.painter().rect_stroke(r.shrink(3.0), 4.0, Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
                }
                let ink = if active { t.accent_text } else { t.icon };
                let name = tl!(m.label());
                let text_w = crate::theme::text_width(ui.painter(), name, &regular(FOOT_NAME), 0.0, ink);
                let icon_x = if named { r.center().x - (text_w + 20.0) / 2.0 + 9.0 } else { r.center().x };
                icons::paint(ui.painter(), Rect::from_center_size(pos2(icon_x, r.center().y), vec2(18.0, 18.0)), icon, ink, ink);
                if named {
                    let at = pos2(icon_x + 11.0, r.center().y);
                    ui.painter().text(at, Align2::LEFT_CENTER, name, regular(FOOT_NAME), if active { t.accent_text } else { t.text });
                }
                app.hit.add("moduleFoot", i as u64, r);
                // For a screen reader: a button, and which module is in view. The module is
                // chosen (AccessKit's `selected`), not switched on, as the Gmail tabs are.
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
                ui.ctx().accesskit_node_builder(resp.id, |node| node.set_selected(active));
                let sc = crate::widgets::shortcut_text(app, id);
                if resp.on_hover_text(format!("{name} ({sc})")).clicked() {
                    let _ = app.run(id, json!({}));
                }
            }
        },
    );
}

/// How deep the pane follows subfolders, and how many rows it lists for one account: a mailbox
/// may come from anywhere, and the pane is laid out again every frame. As many rows as a mailbox
/// holds folders (#991: an IMAP account alone may have 10,000), so that none is left out; only
/// the rows in sight are drawn.
const MAX_DEPTH: usize = 32;
const MAX_ROWS: usize = mailcraft_model::MAX_FOLDERS;

/// The folders of `account` as the pane lists them, depth first, skipping what is under a folded
/// one: each with its depth and whether it has subfolders.
#[cfg(test)]
fn visible(mailbox: &Mailbox, folded: &[Id], account: Id) -> Vec<(Folder, usize, bool)> {
    let mut trees = trees(mailbox, |a| a == account);
    visible_in(trees.remove(&account).unwrap_or_default(), folded)
}

/// The folders of each account that `wanted` names, by account and parent, in one pass over all the
/// folders however many accounts there are, each list in the order the pane shows it.
fn trees(mailbox: &Mailbox, wanted: impl Fn(Id) -> bool) -> HashMap<Id, Tree<'_>> {
    let mut trees: HashMap<Id, Tree> = HashMap::new();
    for f in mailbox.folders.iter().filter(|f| wanted(f.account)) {
        trees.entry(f.account).or_default().entry(f.parent).or_default().push(f);
    }
    for tree in trees.values_mut() {
        for (parent, list) in tree.iter_mut() {
            sort_as_listed(list, parent.is_none());
        }
    }
    trees
}

type Tree<'a> = HashMap<Option<Id>, Vec<&'a Folder>>;

fn visible_in(tree: Tree<'_>, folded: &[Id]) -> Vec<(Folder, usize, bool)> {
    let mut out = Vec::new();
    list_under(&tree, folded, None, 0, &mut out);
    out
}

fn list_under(tree: &HashMap<Option<Id>, Vec<&Folder>>, folded: &[Id], parent: Option<Id>, depth: usize, out: &mut Vec<(Folder, usize, bool)>) {
    if depth > MAX_DEPTH {
        return;
    }
    for f in tree.get(&parent).into_iter().flatten() {
        if out.len() >= MAX_ROWS {
            return;
        }
        let has_children = tree.get(&Some(f.id)).is_some_and(|under| !under.is_empty());
        out.push(((*f).clone(), depth, has_children));
        if has_children && !folded.contains(&f.id) {
            list_under(tree, folded, Some(f.id), depth + 1, out);
        }
    }
}

/// What a folder shows beside its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Count {
    Nothing,
    /// Unread messages: the number, bold.
    Unread(usize),
    /// Everything in the folder, for the folders where all of it waits for something (Drafts,
    /// Outbox) or is on its way out (Junk Email): the number in square brackets.
    Total(usize),
}

impl Count {
    /// A folder of `kind` holding `total` messages, `unread` of them unread.
    pub(crate) fn of(kind: FolderKind, total: usize, unread: usize) -> Count {
        match kind {
            FolderKind::Drafts | FolderKind::Outbox | FolderKind::Junk if total > 0 => Count::Total(total),
            FolderKind::Inbox | FolderKind::Archive | FolderKind::Custom if unread > 0 => Count::Unread(unread),
            // Sent and deleted mail asks for nobody's attention, read or not.
            _ => Count::Nothing,
        }
    }

    /// The number alone, as the pill shows it.
    fn digits(self) -> String {
        match self {
            Count::Nothing => String::new(),
            Count::Unread(n) | Count::Total(n) => n.to_string(),
        }
    }

    #[cfg(test)]
    fn text(self) -> String {
        match self {
            Count::Nothing => String::new(),
            Count::Unread(n) => n.to_string(),
            Count::Total(n) => format!("[{n}]"),
        }
    }
}

/// A chevron centred on `c`, `half` its half-width when it points down (what is under it shows);
/// it points right when that is folded away.
fn chevron(painter: &Painter, c: Pos2, half: f32, open: bool, color: Color32) {
    let points = if open {
        vec![pos2(c.x - half, c.y - half / 2.0), pos2(c.x, c.y + half / 2.0), pos2(c.x + half, c.y - half / 2.0)]
    } else {
        vec![pos2(c.x - half / 2.0, c.y - half), pos2(c.x + half / 2.0, c.y), pos2(c.x - half / 2.0, c.y + half)]
    };
    painter.add(Shape::line(points, Stroke::new(1.5, color)));
}

/// Where a row's highlight goes.
fn highlight(r: Rect) -> Rect {
    Rect::from_min_max(pos2(r.min.x + HIGHLIGHT_LEFT, r.min.y), pos2(r.max.x - HIGHLIGHT_RIGHT, r.max.y))
}

/// A heading of the pane (Favorites, an account): a chevron and the name, `height` tall with the
/// name's line centred `text_y` under the top, and a rule above it when `ruled`. A click folds or
/// unfolds what is under it; `true` when that shows. A `movable` one (an account of the owner's
/// own, beside another, #702) is dragged up or down the pane by it.
fn heading(app: &mut MailApp, ui: &mut Ui, id: Id, name: &str, height: f32, text_y: f32, ruled: bool, movable: bool) -> bool {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let resp = ui.interact(r, ui.scope_id().with(("heading", id)), if movable { Sense::click_and_drag() } else { Sense::click() });
    app.hit.add("folderHeading", id, r);
    if movable {
        resp.dnd_set_drag_payload(AccountDrag(id));
    }
    if ruled {
        let y = r.min.y + 9.5;
        ui.painter().line_segment([pos2(r.min.x + HIGHLIGHT_LEFT, y), pos2(r.max.x - HIGHLIGHT_RIGHT, y)], Stroke::new(1.0, t.border));
    }
    let open = !app.ui.collapsed_folders.contains(&id);
    let y = r.min.y + text_y;
    chevron(ui.painter(), pos2(r.min.x + CHEVRON_X, y), HEADING_CHEVRON, open, if resp.hovered() { t.accent_text } else { t.text });
    // An account's colour (#893): a dot after its name, which leaves room for it.
    let color = app.session.mailbox.account(id).and_then(|a| a.color).filter(|_| id != FAVORITES);
    let room = if color.is_some() { ACCOUNT_DOT_ROOM } else { 0.0 };
    let w =
        paint_clipped(ui.painter(), pos2(r.min.x + NAME_LEFT, y), (r.width() - NAME_LEFT - 8.0 - room).max(0.0), name, regular(HEADING_SIZE), t.text);
    if let Some(color) = color {
        let center = pos2(r.min.x + NAME_LEFT + w + ACCOUNT_DOT_ROOM / 2.0 + 1.0, y + 1.0);
        ui.painter().circle_filled(center, ACCOUNT_DOT_RADIUS, crate::theme::c32(color));
        // (In High Contrast a ring in the text's colour, so that a mid-tone stays in sight.)
        if t.high_contrast {
            ui.painter().circle_stroke(center, ACCOUNT_DOT_RADIUS, Stroke::new(1.0, t.text));
        }
        app.hit.add("accountColor", id, Rect::from_center_size(center, vec2(2.0 * ACCOUNT_DOT_RADIUS, 2.0 * ACCOUNT_DOT_RADIUS)));
    }
    if resp.clicked() {
        let _ = app.execute("ui.toggleFolder", json!({"id": id}));
    }
    // An account's heading (not the Favorites').
    if id != FAVORITES {
        resp.context_menu(|ui| {
            ui.set_min_width(190.0);
            // The keyboard's and the menu's way of a drag (#702), for the accounts that drag.
            if movable {
                let own = owners(&app.session.mailbox);
                for (label, up) in [(tl!("Move Up"), true), (tl!("Move Down"), false)] {
                    let before = move_before(&own, id, up);
                    if ui.add_enabled(before.is_some(), egui::Button::new(label).min_size(vec2(180.0, 0.0))).clicked() {
                        if let Some(before) = before {
                            let _ = app.run("account.move", json!({"id": id, "before": before}));
                        }
                        ui.close();
                    }
                }
                ui.separator();
            }
            if ui.add(egui::Button::new(tl!("Properties…")).min_size(vec2(180.0, 0.0))).clicked() {
                let _ = app.run("folder.properties", json!({"account": id}));
                ui.close();
            }
        });
    }
    open
}

/// An account being dragged up or down the folder pane by its heading (#702).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AccountDrag(pub Id);

/// The owner's own accounts, top to bottom: what a drag puts in another order. A shared mailbox
/// goes with the account it is opened through.
fn owners(mb: &Mailbox) -> Vec<Id> {
    mb.accounts_in_order().into_iter().filter(|a| !a.is_shared()).map(|a| a.id).collect()
}

/// What `account.move` is told to put `account` one place up (`up`) or down among the owner's
/// own accounts `own`: the account it goes before, `None` inside for the end. `None` at the top
/// for Move Up, at the bottom for Move Down, and for an account that isn't among them.
fn move_before(own: &[Id], account: Id, up: bool) -> Option<Option<Id>> {
    let at = own.iter().position(|a| *a == account)?;
    if up {
        own.get(at.checked_sub(1)?).map(|a| Some(*a))
    } else {
        own.get(at.checked_add(1)?)?;
        Some(own.get(at.saturating_add(2)).copied())
    }
}

/// How far the folder pane scrolls this frame while an account is dragged with the pointer at
/// height `y` (#702): towards the top within a row of the top of the part in view, `pane`,
/// towards the bottom within a row of its bottom, faster the nearer the edge; else nothing. A
/// positive amount moves the rows down (shows what is above).
fn edge_scroll(pane: Rect, y: f32) -> f32 {
    const MOST: f32 = 12.0;
    let (top, bottom) = (y - pane.min.y, pane.max.y - y);
    if top < ROW_H {
        MOST * (1.0 - top.max(0.0) / ROW_H)
    } else if bottom < ROW_H {
        -MOST * (1.0 - bottom.max(0.0) / ROW_H)
    } else {
        0.0
    }
}

/// Where an account of the owner's own starts in the pane, for a drag of one (#702): its
/// heading's top, and the height a line there is drawn at (on the rule above the heading).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Block {
    account: Id,
    top: f32,
    line: f32,
}

/// The gap between the owner's accounts that a pointer at height `y` points at: 0 before the
/// first, `blocks.len()` after the last. Over an account (its heading, its folders, its shared
/// mailboxes) the gap above it in its upper half and the one below in its lower half; above the
/// first (over Favorites) the first gap, below the last the last. `end` is where the last ends.
fn gap_at(blocks: &[Block], end: f32, y: f32) -> usize {
    for (n, block) in blocks.iter().enumerate() {
        let bottom = blocks.get(n + 1).map_or(end, |next| next.top);
        if y < block.top {
            return n;
        }
        if y < bottom {
            return if y < (block.top + bottom) / 2.0 { n } else { n + 1 };
        }
    }
    blocks.len()
}

/// While an account is dragged by its heading (#702): over the pane, a line in the gap between
/// two accounts where it would go, and on letting go there the move (`account.move`). Letting go
/// where it already is changes nothing, and so do Escape and letting go outside the pane.
fn account_drop(app: &mut MailApp, ui: &mut Ui, blocks: &[Block], end: f32) {
    let Some(drag) = egui::DragAndDrop::payload::<AccountDrag>(ui.ctx()) else { return };
    let Some(pos) = ui.ctx().pointer_interact_pos() else { return };
    let Some(from) = blocks.iter().position(|b| b.account == drag.0) else { return };
    // (The part of the pane in view: the scroll area's.) The pointer is over the pane itself, not
    // over a window or an agent's question on top of it.
    let pane = ui.clip_rect();
    if !ui.rect_contains_pointer(pane) {
        return;
    }
    crate::drag_pointer::note_folder(ui.ctx(), true);
    // Near the top or the bottom of what is in view the pane scrolls, to reach an account out of view.
    let step = edge_scroll(pane, pos.y);
    if step != 0.0 {
        ui.scroll_with_delta(vec2(0.0, step));
        ui.ctx().request_repaint();
    }
    let gap = gap_at(blocks, end, pos.y);
    let t = Tokens::get(ui.ctx());
    let y = blocks.get(gap).map_or(end + 2.0, |b| b.line);
    let (left, right) = (pane.min.x + HIGHLIGHT_LEFT, pane.max.x - HIGHLIGHT_RIGHT);
    ui.painter().line_segment([pos2(left + 3.0, y), pos2(right, y)], Stroke::new(2.0, t.accent));
    ui.painter().circle_stroke(pos2(left + 3.0, y), 3.0, Stroke::new(2.0, t.accent));
    app.hit.add("accountGap", gap as u64, Rect::from_min_max(pos2(left, y - 3.0), pos2(right, y + 3.0)));
    if ui.input(|i| i.pointer.any_released()) {
        egui::DragAndDrop::clear_payload(ui.ctx());
        if gap != from && gap != from + 1 {
            let before = blocks.get(gap).map(|b| b.account);
            let _ = app.run("account.move", json!({"id": drag.0, "before": before}));
        }
    }
}

/// While an account is being dragged: a small label by the pointer with its name.
fn account_ghost(app: &MailApp, ui: &Ui) {
    let Some(drag) = egui::DragAndDrop::payload::<AccountDrag>(ui.ctx()) else { return };
    let Some(pos) = ui.ctx().pointer_interact_pos() else { return };
    let Some(a) = app.session.mailbox.account(drag.0) else { return };
    let t = Tokens::get(ui.ctx());
    let name = if a.name.trim().is_empty() { tl!("Mailbox").to_string() } else { a.name.clone() };
    egui::Area::new(egui::Id::unique("account_drag_ghost")).order(egui::Order::Tooltip).fixed_pos(pos + vec2(14.0, 16.0)).interactable(false).show(
        ui.ctx(),
        |ui| {
            egui::Frame::NONE
                .fill(t.menu)
                .stroke(Stroke::new(1.0, t.border_strong))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(name).font(regular(12.0)).color(t.text));
                });
        },
    );
}

/// Where the folder in view was last picked: `true` in its account's list, `false` under
/// Favorites (and before any pick). A folder listed in both is highlighted in that one place.
fn picked_in_list(ui: &Ui) -> bool {
    ui.data(|d| d.get_temp::<bool>(egui::Id::unique("folder_picked_in_list"))).unwrap_or(false)
}

/// A folder's row, `selected` when it is the one the folder in view is highlighted in. `key` tells
/// the Favorites' rows (`fav`) from the accounts' (`tree`).
fn folder_row(app: &mut MailApp, ui: &mut Ui, f: &Folder, depth: usize, has_children: bool, key: &str, count: Count, selected: bool) {
    // Under Favorites a shared mailbox's folder says whose it is (#70), and so does one whose name
    // another account's folder has too (two Inboxes); in its tree, its name.
    let name = if key == "fav" { result_folder_label(&app.session.mailbox, f) } else { folder_name(f).to_string() };
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let resp = ui.interact(r, ui.scope_id().with((key, f.id)), Sense::click());
    app.hit.add("folder", f.id, r);
    let row = highlight(r).shrink2(vec2(0.0, ROW_HIGHLIGHT_INSET));
    if selected {
        ui.painter().rect_filled(row, 6.0, t.accent_soft);
        ui.painter().rect_filled(Rect::from_center_size(pos2(row.min.x + 1.5, row.center().y), vec2(3.0, 14.0)), 1.5, t.selection_edge);
    } else if resp.hovered() {
        // Quieter than on the ribbon: the pane is darker, and the selected row must stay the one that stands out.
        ui.painter().rect_filled(row, 6.0, t.hover_nav);
    }
    let x = r.min.x + depth.min(MAX_INDENT_DEPTH) as f32 * INDENT;
    // The chevron has its own click target, only on a folder with something under it.
    if has_children {
        let tr = Rect::from_min_max(pos2(x + HIGHLIGHT_LEFT, r.min.y), pos2(x + NAME_LEFT, r.max.y));
        let tresp = ui.interact(tr, ui.scope_id().with((key, "twisty", f.id)), Sense::click());
        let open = !app.ui.collapsed_folders.contains(&f.id);
        chevron(ui.painter(), pos2(x + CHEVRON_X, r.center().y), ROW_CHEVRON, open, if tresp.hovered() { t.accent_text } else { t.text });
        if tresp.clicked() {
            let _ = app.execute("ui.toggleFolder", json!({"id": f.id}));
        }
    }
    let y = r.center().y;
    // The count, flush right, in a pill: an unread count in the accent's soft colour, a total
    // (Drafts, Junk) in a quiet grey one.
    let mut name_end = r.max.x - COUNT_RIGHT;
    if count != Count::Nothing {
        let (fill, ink) = if matches!(count, Count::Unread(_)) { (t.accent_soft, t.accent_soft_text) } else { (t.pill_quiet, t.text_dim) };
        let fill = if selected && matches!(count, Count::Unread(_)) { t.surface } else { fill };
        let galley = ui.painter().layout_no_wrap(count.digits(), bold(NAME_SIZE - 1.0), ink);
        let w = (galley.size().x + 12.0).max(20.0);
        let pill = Rect::from_center_size(pos2(r.max.x - COUNT_RIGHT + 4.0 - w / 2.0, r.center().y), vec2(w, 18.0));
        ui.painter().rect_filled(pill, 9.0, fill);
        ui.painter().galley(pill.center() - galley.size() / 2.0, galley, ink);
        name_end = pill.min.x - 6.0;
    }
    let font = if selected { bold(NAME_SIZE) } else { regular(NAME_SIZE) };
    paint_clipped(
        ui.painter(),
        pos2(x + NAME_LEFT, y),
        (name_end - x - NAME_LEFT).max(0.0),
        &name,
        font,
        if selected { t.accent_soft_text } else { t.text },
    );
    if resp.clicked() {
        ui.data_mut(|d| d.insert_temp(egui::Id::unique("folder_picked_in_list"), key == "tree"));
        let _ = app.run("folder.select", json!({"id": f.id}));
    }
    // Messages dragged out of the list are filed in the folder they are let go over.
    if let Some(drag) = resp.dnd_hover_payload::<crate::mail_view::MessageDrag>() {
        let takes = !crate::drag_pointer::movable(&app.session.mailbox, &drag.0, f.id).is_empty();
        crate::drag_pointer::note_folder(ui.ctx(), takes);
        if takes {
            ui.painter().rect_stroke(row, 0.0, Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
        }
    }
    if let Some(drag) = resp.dnd_release_payload::<crate::mail_view::MessageDrag>() {
        // Only what isn't here already.
        let ids = crate::drag_pointer::movable(&app.session.mailbox, &drag.0, f.id);
        if !ids.is_empty() {
            let _ = app.run("mail.move", json!({"ids": ids, "folder": f.id}));
        }
    }
    // (One of the server's own, Conversation History say, is left as it is, and takes no new
    // folder either: #756.)
    let (id, custom, favorite, kind) = (f.id, f.kind == FolderKind::Custom && !f.server_owned(), f.favorite, f.kind);
    let takes_folders = !(f.kind == FolderKind::Custom && f.server_owned());
    // An account whose Favorites are Outlook's has them changed in Outlook: nothing to do here.
    let from_outlook = mailcraft_engine::cmd::folder::favorites_from_server(&app.session, f.account);
    resp.context_menu(|ui| {
        ui.set_min_width(190.0);
        let item = |ui: &mut Ui, app: &mut MailApp, label: &str, cmd: &str, params: serde_json::Value, enabled: bool| {
            if ui.add_enabled(enabled, egui::Button::new(tl!(label)).min_size(vec2(180.0, 0.0))).clicked() {
                let _ = app.run(cmd, params);
                ui.close();
            }
        };
        item(ui, app, "New Folder…", "ui.dialog", json!({"name": "folder", "mode": "new", "parent": id}), takes_folders);
        // (Not one that is or holds what a JMAP account's server has, which stays for now.)
        let renamable = custom && !app.session.mailbox.renaming_stays_on_server(id);
        item(ui, app, "Rename Folder…", "ui.dialog", json!({"name": "folder", "mode": "rename", "id": id}), renamable);
        let deletable = custom && !app.session.mailbox.folder_stays_on_server(id);
        item(ui, app, "Delete Folder", "folder.delete", json!({"id": id}), deletable);
        ui.separator();
        item(ui, app, "Mark All as Read", "mail.markAllRead", json!({"folder": id}), true);
        item(ui, app, "Run Rules Now", "rules.run", json!({"folder": id}), true);
        if matches!(kind, FolderKind::Deleted | FolderKind::Junk) {
            let emptiable = !app.session.mailbox.emptying_stays_on_server(id);
            item(ui, app, "Empty Folder", "folder.empty", json!({"id": id}), emptiable);
        }
        ui.separator();
        if from_outlook {
            item(ui, app, "Favorites Are Set in Outlook", "folder.favorite", json!({"id": id}), false);
        } else {
            item(ui, app, if favorite { "Remove from Favorites" } else { "Add to Favorites" }, "folder.favorite", json!({"id": id}), true);
        }
        ui.separator();
        item(ui, app, "Properties…", "folder.properties", json!({"id": id}), true);
    });
}

/// What the window calls one of Gmail's categories (#911).
pub(crate) fn gmail_name(category: GmailCategory) -> &'static str {
    match category {
        GmailCategory::Primary => tl!("Primary"),
        GmailCategory::Social => tl!("Social"),
        GmailCategory::Promotions => tl!("Promotions"),
        GmailCategory::Updates => tl!("Updates"),
        GmailCategory::Forums => tl!("Forums"),
    }
}

/// What a row of one of Gmail's categories is known by to `ui.inspect`, with its Inbox's id.
fn gmail_hit(category: GmailCategory) -> &'static str {
    match category {
        GmailCategory::Primary => "gmailPrimary",
        GmailCategory::Social => "gmailSocial",
        GmailCategory::Promotions => "gmailPromotions",
        GmailCategory::Updates => "gmailUpdates",
        GmailCategory::Forums => "gmailForums",
    }
}

/// The row of one of Gmail's categories under a Gmail account's Inbox (#911), a level further in
/// than the Inbox (`depth`): its name and its unread count, `selected` when the list shows it.
/// It is a view of the Inbox, no folder: a click shows its mail, and it has nothing to rename or
/// delete, so no menu. Mail let go over it is moved to that category where moves between Gmail's
/// categories are on ([`crate::gmail_move`]), and stays where it was otherwise.
fn gmail_row(app: &mut MailApp, ui: &mut Ui, inbox: Id, depth: usize, category: GmailCategory, unread: usize, selected: bool) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let resp = ui.interact(r, ui.scope_id().with(("gmail", inbox, category.code())), Sense::click());
    app.hit.add(gmail_hit(category), inbox, r);
    let row = highlight(r).shrink2(vec2(0.0, ROW_HIGHLIGHT_INSET));
    if selected {
        ui.painter().rect_filled(row, 6.0, t.accent_soft);
        ui.painter().rect_filled(Rect::from_center_size(pos2(row.min.x + 1.5, row.center().y), vec2(3.0, 14.0)), 1.5, t.selection_edge);
    } else if resp.hovered() {
        ui.painter().rect_filled(row, 6.0, t.hover_nav);
    }
    // Reached with Tab or the arrow keys (Enter or Space shows it): where the keyboard is.
    if resp.has_focus() {
        ui.painter().rect_stroke(row, 6.0, Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
    }
    let x = r.min.x + depth.min(MAX_INDENT_DEPTH) as f32 * INDENT;
    let y = r.center().y;
    let mut name_end = r.max.x - COUNT_RIGHT;
    if unread > 0 {
        let fill = if selected { t.surface } else { t.accent_soft };
        let galley = ui.painter().layout_no_wrap(unread.to_string(), bold(NAME_SIZE - 1.0), t.accent_soft_text);
        let w = (galley.size().x + 12.0).max(20.0);
        let pill = Rect::from_center_size(pos2(r.max.x - COUNT_RIGHT + 4.0 - w / 2.0, r.center().y), vec2(w, 18.0));
        ui.painter().rect_filled(pill, 9.0, fill);
        ui.painter().galley(pill.center() - galley.size() / 2.0, galley, t.accent_soft_text);
        name_end = pill.min.x - 6.0;
    }
    let name = gmail_name(category);
    let font = if selected { bold(NAME_SIZE) } else { regular(NAME_SIZE) };
    paint_clipped(
        ui.painter(),
        pos2(x + NAME_LEFT, y),
        (name_end - x - NAME_LEFT).max(0.0),
        name,
        font,
        if selected { t.accent_soft_text } else { t.text },
    );
    // Read out as its name and how many of its messages are unread.
    let said = if unread > 0 { format!("{name}, {}", tln!(unread, "{n} unread", "{n} unread")) } else { name.to_string() };
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::TreeItem, true, selected, &said));
    if resp.clicked() {
        let _ = app.run("folder.select", json!({"id": inbox, "gmail": category.code()}));
    }
    crate::gmail_move::drop_target(app, ui, &resp, row, category);
}

/// A sentence under the Favorites heading that is longer than a row: wrapped to the pane, three
/// rows at most, in the quiet colour. Where it was put.
fn quiet_lines(ui: &mut Ui, text: &str) -> Rect {
    let t = Tokens::get(ui.ctx());
    let width = (ui.available_width() - NAME_LEFT - COUNT_RIGHT).max(40.0);
    let mut job = egui::text::LayoutJob::simple(text.to_string(), regular(NAME_SIZE), t.text_dim, width);
    job.wrap.max_rows = 3;
    let said = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), (said.size().y + 8.0).max(ROW_H)), Sense::hover());
    ui.painter().galley(pos2(r.min.x + NAME_LEFT, r.min.y + 4.0), said, t.text_dim);
    r
}

/// A link under such a sentence, a row of its own: what the owner can do about it.
fn quiet_link(app: &mut MailApp, ui: &mut Ui, account: Id, step: crate::favorites_sync::Step) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let resp = ui.interact(highlight(r), ui.scope_id().with(("favorites_step", account, step.code())), Sense::click());
    let kind = match step {
        crate::favorites_sync::Step::CopyMessage => "favoritesCopyMessage",
        crate::favorites_sync::Step::Help => "favoritesHelp",
    };
    app.hit.add(kind, account, r);
    if resp.hovered() {
        ui.painter().rect_filled(highlight(r), 0.0, t.hover_nav);
    }
    let width = (r.max.x - COUNT_RIGHT - r.min.x - NAME_LEFT).max(0.0);
    paint_clipped(ui.painter(), pos2(r.min.x + NAME_LEFT, r.center().y), width, tl!(step.label()), regular(NAME_SIZE), t.accent_text);
    if resp.on_hover_text(tl!(step.about())).clicked() {
        crate::favorites_sync::step_taken(app, account, step);
    }
}

/// What is said under the Favorites heading, when there is something to say of an account's
/// Favorites in Outlook ([`crate::favorites_sync`]): the offer to show them, why they can't be
/// had, or why the last look came to nothing, with "Try again" after it: one row. When what
/// stands in the way is somebody else's to change (an administrator has to approve it), the
/// sentence that says so, and under it a row for each thing the owner can do about it.
fn favorites_note(app: &mut MailApp, ui: &mut Ui, account: Id, note: &Note) {
    let t = Tokens::get(ui.ctx());
    if matches!(note, Note::Approval) {
        let r = quiet_lines(ui, tl!(note.text()));
        app.hit.add("favoritesNote", account, r);
        let about = tl!(
            "Your organisation keeps this permission for its administrators to give. Once one of them has approved it, choose it again here. Until then the Favorites kept in MailCraft are shown."
        );
        ui.interact(r, ui.scope_id().with(("favorites_note", account)), Sense::hover()).on_hover_text(about);
        for step in app.favorites_steps(note) {
            quiet_link(app, ui, account, step);
        }
        return;
    }
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let y = r.center().y;
    let left = r.min.x + NAME_LEFT;
    let width = (r.max.x - COUNT_RIGHT - left).max(0.0);
    let font = regular(NAME_SIZE);
    match note {
        // The whole line is the offer.
        Note::Offer => {
            let resp = ui.interact(highlight(r), ui.scope_id().with(("favorites_offer", account)), Sense::click());
            app.hit.add("favoritesOffer", account, r);
            if resp.hovered() {
                ui.painter().rect_filled(highlight(r), 0.0, t.hover_nav);
            }
            paint_clipped(ui.painter(), pos2(left, y), width, tl!("Show Outlook's Favorites"), font, t.accent_text);
            // Signing in with Microsoft once more, for that alone.
            let about = tl!(
                "Sign in with Microsoft once more to let MailCraft read the folders you keep under Favorites in Outlook. They are only read, never changed, and everything else stays as it is."
            );
            if resp.on_hover_text(about).clicked()
                && let Err(e) = app.run("ui.connectFavorites", json!({"account": account}))
            {
                app.status(e);
            }
        }
        // (Said above, in more than a row.)
        Note::Approval => {}
        Note::Refused(why) => {
            let resp = ui.interact(highlight(r), ui.scope_id().with(("favorites_note", account)), Sense::hover());
            app.hit.add("favoritesNote", account, r);
            paint_clipped(ui.painter(), pos2(left, y), width, tl!(why), font, t.text_dim);
            resp.on_hover_text(tl!(
                "Outlook's Favorites can't be shown for this account, so the ones kept in MailCraft are. File › Options can ask again."
            ));
        }
        // Why, in a few words, and the way to ask again right after it.
        Note::Failed(why) => {
            app.hit.add("favoritesNote", account, r);
            let again = tl!("Try again");
            let again_w = crate::theme::text_width(ui.painter(), again, &font, 0.0, t.accent_text);
            let used = paint_clipped(ui.painter(), pos2(left, y), (width - again_w - 10.0).max(0.0), tl!(why), font.clone(), t.text_dim);
            let link = Rect::from_min_size(pos2(left + used + if used > 0.0 { 10.0 } else { 0.0 }, r.min.y), vec2(again_w, ROW_H));
            let resp = ui.interact(link, ui.scope_id().with(("favorites_retry", account)), Sense::click());
            app.hit.add("favoritesRetry", account, link);
            let ink = if resp.hovered() { t.accent } else { t.accent_text };
            ui.painter().text(pos2(link.min.x, y), Align2::LEFT_CENTER, again, font, ink);
            if resp.on_hover_text(tl!("Read the Favorites from Outlook again now")).clicked()
                && let Err(e) = app.run("ui.favorites", json!({"account": account, "refresh": true}))
            {
                app.status(e);
            }
        }
    }
}

/// The pane as a sheet lying on the window: the panel colour with a line along its top and left,
/// rounded where the two meet beside the module bar and under the ribbon.
fn sheet(painter: &Painter, r: Rect, t: &Tokens) {
    let radius = f32::from(SHEET_CORNER);
    painter.rect_filled(r, CornerRadius { nw: SHEET_CORNER, ne: 0, sw: 0, se: 0 }, t.panel);
    // Up the left edge, round the corner, along the top: half a pixel in, so the line is crisp.
    let (left, top) = (r.min.x + 0.5, r.min.y + 0.5);
    let centre = pos2(left + radius, top + radius);
    let mut line = vec![pos2(left, r.max.y)];
    line.extend((0..=8).map(|i| {
        let a = (180.0 + 90.0 * i as f32 / 8.0).to_radians();
        pos2(centre.x + radius * a.cos(), centre.y + radius * a.sin())
    }));
    line.push(pos2(r.max.x, top));
    painter.add(Shape::line(line, Stroke::new(1.0, t.border_strong)));
}

/// The top of the pane: the button that puts the pane away.
fn pane_top(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    // The line along the pane's top, and a little air under it.
    ui.add_space(4.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let button = Rect::from_center_size(pos2(r.max.x - 14.0, r.center().y), vec2(20.0, 20.0));
    let resp = ui.interact(button, ui.scope_id().with("hide_pane"), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(button, 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
    }
    let c = button.center();
    ui.painter().add(Shape::line(vec![pos2(c.x + 1.75, c.y - 3.4), pos2(c.x - 1.65, c.y), pos2(c.x + 1.75, c.y + 3.4)], Stroke::new(1.5, t.text)));
    app.hit.add("folderPaneHide", 0, button);
    if resp.on_hover_text(tl!("Hide the folder pane")).clicked() {
        let _ = app.run("view.folderPane", json!({"value": false}));
    }
}

/// What is left of the folder pane while it is hidden: a narrow strip with the button that
/// brings it back, where the button that hid it was.
pub fn folder_pane_strip(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::left("folders_hidden").exact_size(STRIP_W).resizable(false).frame(egui::Frame::NONE.fill(t.canvas)).show(ui, |ui| {
        sheet(ui.painter(), ui.max_rect(), &t);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        ui.add_space(4.0);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
        let button = Rect::from_center_size(r.center(), vec2(20.0, 20.0));
        let resp = ui.interact(button, ui.scope_id().with("show_pane"), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(button, 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
        }
        let c = button.center();
        ui.painter()
            .add(Shape::line(vec![pos2(c.x - 1.75, c.y - 3.4), pos2(c.x + 1.65, c.y), pos2(c.x - 1.75, c.y + 3.4)], Stroke::new(1.5, t.text)));
        app.hit.add("folderPaneShow", 0, button);
        if resp.on_hover_text(tl!("Show the folder pane")).clicked() {
            let _ = app.run("view.folderPane", json!({"value": true}));
        }
    });
}

/// The folder pane: favourites, then every account's folders.
pub fn folder_pane(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let panel = egui::Panel::left("folders")
        .default_size(app.ui.folder_pane_width)
        .size_range(160.0..=420.0)
        .resizable(true)
        // In the dark the pane and the list beside it are told apart by their own colours; on a
        // light window they are nearly one white, and a line parts them.
        .show_separator_line(t.pane_rule)
        .frame(egui::Frame::NONE.fill(t.canvas));
    let shown = panel.show(ui, |ui| {
        // (After the sheet: the pane's background would paint over the row.)
        sheet(ui.painter(), ui.max_rect(), &t);
        module_foot(app, ui);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        pane_top(app, ui, &t);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // (Counted once per change to the mailbox, not at every frame: `Session::folder_counts`.)
            let counts = app.session.folder_counts();
            let count = |f: &Folder| {
                let held = counts.folder(f.id);
                Count::of(f.kind, held.total, held.unread)
            };
            // What is listed: the favourites, and under each account what isn't folded away.
            let folded = app.ui.collapsed_folders.clone();
            // The favourites as the engine lists them: Outlook's own, in Outlook's order, for an
            // account whose server has them; else the ones that are marked here.
            let favorites: Vec<Folder> =
                mailcraft_engine::cmd::folder::favorites(&app.session).into_iter().filter_map(|id| app.session.mailbox.folder(id).cloned()).collect();
            // And what there is to say of Outlook's, in one quiet line under the heading.
            let note = app.favorites_note();
            // (A shared mailbox right after the account it is opened through, #70.)
            // (The folders are grouped by account once, not once for each account.)
            let mut trees = trees(&app.session.mailbox, |a| !folded.contains(&a));
            let lists: Vec<(Id, String, Vec<(Folder, usize, bool)>)> = app
                .session
                .mailbox
                .accounts_in_order()
                .into_iter()
                .map(|a| {
                    let rows = visible_in(trees.remove(&a.id).unwrap_or_default(), &folded);
                    (a.id, heading_name(app, a), rows)
                })
                .collect();
            // The folder in view is highlighted once: under Favorites when it shows there, unless
            // it was picked in its account's list and shows there too. While the list shows one of
            // Gmail's categories of it (#911), that category's row is, and the Inbox's isn't; when
            // they are tabs above the list instead (`view.gmailCategories`), the Inbox's is.
            let gmail_tabs = app.session.view.gmail_tabs;
            let gmail_now = mailcraft_engine::gmail::in_view(&app.session);
            let current = if gmail_now.is_some() && !gmail_tabs { 0 } else { app.session.view.folder };
            let gmail_current = app.session.view.folder;
            let among_favorites = !folded.contains(&FAVORITES) && favorites.iter().any(|f| f.id == current);
            let in_a_list = lists.iter().any(|(_, _, rows)| rows.iter().any(|(f, _, _)| f.id == current));
            let favorite_selected = among_favorites && !(in_a_list && picked_in_list(ui));
            // The owner's own accounts are dragged up or down by their headings (#702), when there
            // are two at least; a shared mailbox goes with its account.
            let own = owners(&app.session.mailbox);
            let movable: HashSet<Id> = if own.len() > 1 { own.into_iter().collect() } else { HashSet::new() };
            // The first heading sits right under the top of the pane; each later one starts a
            // new block, with a rule above it.
            let pane_top = ui.cursor().min.y;
            let mut first = true;
            let mut head = |app: &mut MailApp, ui: &mut Ui, id: Id, name: &str| {
                if std::mem::take(&mut first) {
                    heading(app, ui, id, name, ROW_H, 7.5, false, movable.contains(&id))
                } else {
                    heading(app, ui, id, name, 2.0 * ROW_H, 34.5, true, movable.contains(&id))
                }
            };
            if (!favorites.is_empty() || note.is_some()) && head(app, ui, FAVORITES, tl!("Favorites")) {
                if let Some((account, note)) = &note {
                    favorites_note(app, ui, *account, note);
                }
                for f in &favorites {
                    folder_row(app, ui, f, 0, false, "fav", count(f), favorite_selected && f.id == current);
                }
            }
            // Where each of the owner's accounts starts, for a drag of one.
            let mut blocks: Vec<Block> = Vec::new();
            for (account, name, rows) in &lists {
                if movable.contains(account) {
                    let top = ui.cursor().min.y;
                    // (On the rule above a heading; at the top of the pane, just under it.)
                    let line = if top <= pane_top { top + 1.0 } else { top + 9.5 };
                    blocks.push(Block { account: *account, top, line });
                }
                if !head(app, ui, *account, if name.is_empty() { tl!("Mailbox") } else { name }) {
                    continue;
                }
                for (f, depth, has_children) in rows {
                    // A Gmail account's Inbox has Gmail's categories under it, folded with it,
                    // unless they are tabs above its list.
                    let gmail = !gmail_tabs && f.kind == FolderKind::Inbox && mailcraft_engine::gmail::listed_under(&mut app.session, f.id);
                    // A row out of sight takes its place and is not drawn (#991: thousands of
                    // folders), but is still known where it is.
                    let top = ui.cursor().min.y;
                    let clip = ui.clip_rect();
                    if !gmail && (top > clip.max.y || top + ROW_H < clip.min.y) {
                        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
                        app.hit.add("folder", f.id, r);
                        continue;
                    }
                    folder_row(app, ui, f, *depth, *has_children || gmail, "tree", count(f), !favorite_selected && f.id == current);
                    if gmail && !app.ui.collapsed_folders.contains(&f.id) {
                        for category in GmailCategory::ALL {
                            let unread = counts.gmail(f.id, category).unread;
                            let selected = f.id == gmail_current && gmail_now == Some(category);
                            gmail_row(app, ui, f.id, depth + 1, category, unread, selected);
                        }
                    }
                }
            }
            let end = ui.cursor().min.y;
            account_drop(app, ui, &blocks, end);
            ui.add_space(8.0);
        });
        account_ghost(app, ui);
    });
    app.ui.folder_pane_width = shown.response.rect.width();
}

/// What the window calls a folder: in English always its own name; in another interface language,
/// a folder of a standard kind (Inbox, Sent Items…) still called by its default name, or one the
/// server has as its folder of that kind (whatever the server calls it), by the language's name
/// for that kind, and the local folder imported mail goes to by the language's word. A folder the
/// owner named keeps its name, and the name kept in the mailbox and on the server never changes.
pub(crate) fn folder_name(f: &Folder) -> &str {
    if crate::i18n::current() == crate::i18n::Lang::English {
        return &f.name;
    }
    match named_by(f) {
        Some(Named::Imported) => tl!("Imported"),
        Some(Named::Kind(kind)) => crate::i18n::current().folder(kind.default_name()),
        None => &f.name,
    }
}

/// What gives a folder its name in another interface language than English ([`folder_name`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Named {
    /// The local folder imported mail goes to ([`mailcraft_model::IMPORTED`]).
    Imported,
    /// Its kind.
    Kind(FolderKind),
}

/// `None`: the folder's own name.
fn named_by(f: &Folder) -> Option<Named> {
    let on_server = !f.remote.is_empty() || f.server.is_some();
    match f.kind {
        FolderKind::Custom => (!on_server && f.name == mailcraft_model::IMPORTED).then_some(Named::Imported),
        kind => (on_server || f.name == kind.default_name()).then_some(Named::Kind(kind)),
    }
}

/// A folder's place in its account's tree (`Inbox/Projects`), each folder by [`folder_name`].
pub(crate) fn folder_path(mb: &Mailbox, id: Id) -> String {
    let mut parts = Vec::new();
    let mut cur = mb.folder(id);
    while let Some(f) = cur {
        parts.push(folder_name(f));
        // No deeper than a mailbox lets a tree go, however a file tangles it.
        if parts.len() > 32 {
            break;
        }
        cur = f.parent.and_then(|p| mb.folder(p));
    }
    parts.reverse();
    parts.join("/")
}

/// What a folder is called where it is listed away from its own account's tree (under Favorites,
/// in the bar over a search's results): its name, and for a folder of a shared mailbox the
/// mailbox it is in, so that its Inbox is never taken for the owner's (#70).
pub(crate) fn folder_label(mb: &Mailbox, f: &Folder) -> String {
    let Some(a) = mb.account(f.account).filter(|a| a.is_shared()) else { return folder_name(f).to_string() };
    let mailbox = if a.name.trim().is_empty() { a.email.trim() } else { a.name.trim() };
    crate::i18n::fmt(tl!("{folder} - {mailbox}"), &[("folder", folder_name(f)), ("mailbox", mailbox)])
}

/// A folder's name on a row of a search of every mailbox (#195): [`folder_label`], and for a
/// folder whose name a folder of another account of the owner's has too (two Inboxes), its
/// account's name after it, so the row says which one. Short otherwise: the name alone. A shared
/// mailbox's folders are named with their mailbox already, so they make no twin.
pub(crate) fn result_folder_label(mb: &Mailbox, f: &Folder) -> String {
    let shared = mb.account(f.account).is_some_and(|a| a.is_shared());
    let own = |id: Id| mb.account(id).is_some_and(|a| !a.is_shared());
    let twin = || mb.folders.iter().any(|o| o.account != f.account && own(o.account) && o.name.eq_ignore_ascii_case(&f.name));
    match mb.account(f.account) {
        Some(a) if !shared && twin() => {
            let account = if a.name.trim().is_empty() { a.email.trim() } else { a.name.trim() };
            crate::i18n::fmt(tl!("{folder} - {mailbox}"), &[("folder", folder_name(f)), ("mailbox", account)])
        }
        _ => folder_label(mb, f),
    }
}

/// What an account's heading in the folder pane says: its name, and for a shared mailbox
/// Microsoft won't open for the owner, No access (#70; the mail already here stays readable).
pub(crate) fn heading_name(app: &MailApp, a: &mailcraft_model::Account) -> String {
    if app.shared_refused.contains_key(&a.id) { crate::i18n::fmt(tl!("{account} · No access"), &[("account", &a.name)]) } else { a.name.clone() }
}

#[cfg(test)]
#[path = "nav_tests.rs"]
pub(crate) mod tests;
