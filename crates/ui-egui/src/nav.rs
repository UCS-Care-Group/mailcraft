// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Getting around: the module bar (Mail, Calendar, People, Tasks) down the left edge, and the
//! folder pane beside it in Mail.

use std::collections::HashMap;

use egui::{Align2, Color32, CornerRadius, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use mailcraft_engine::Module;
use mailcraft_engine::cmd::folder::sort_as_listed;
use mailcraft_model::{Folder, FolderKind, Id, Mailbox};
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
const NAME_LEFT: f32 = 22.0;
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
/// A line of text is centred by its line box; the eye centres it by its lower-case letters, which
/// sit this much below the middle of the box.
const TEXT_RISE: f32 = 1.5;
/// How round the pane's corner is where it meets the module bar and the ribbon.
const SHEET_CORNER: u8 = 8;
/// How wide the strip is that stays when the pane is hidden.
const STRIP_W: f32 = 30.0;

/// The id the Favorites heading is folded under in `UiState::collapsed_folders`, beside the
/// folders' ids and the accounts' (an account's heading folds under the account's own id).
/// Accounts and folders are numbered from 1 in one sequence, so none of the three collide.
const FAVORITES: Id = 0;

/// The module bar. It runs from under the title bar down to the status bar, beside the tabs and
/// the ribbon as well as the panes, so the window lays it out before the ribbon.
pub fn module_bar(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::left("modules").exact_size(RAIL_W).resizable(false).show_separator_line(false).frame(egui::Frame::NONE.fill(t.nav)).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        for m in Module::ALL {
            let (icon, id) = match m {
                Module::Mail => ("mail", "view.mail"),
                Module::Calendar => ("calendar", "view.calendar"),
                Module::People => ("people", "view.people"),
                Module::Tasks => ("tasks", "view.tasks"),
            };
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
    });
}

/// How deep the pane follows subfolders, and how many rows it lists for one account: a mailbox
/// may come from anywhere, and the pane is laid out again every frame.
const MAX_DEPTH: usize = 32;
const MAX_ROWS: usize = 4000;

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
/// unfolds what is under it; `true` when that shows.
fn heading(app: &mut MailApp, ui: &mut Ui, id: Id, name: &str, height: f32, text_y: f32, ruled: bool) -> bool {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let resp = ui.interact(r, ui.scope_id().with(("heading", id)), Sense::click());
    app.hit.add("folderHeading", id, r);
    if ruled {
        let y = r.min.y + 9.5;
        ui.painter().line_segment([pos2(r.min.x + HIGHLIGHT_LEFT, y), pos2(r.max.x - HIGHLIGHT_RIGHT, y)], Stroke::new(1.0, t.border));
    }
    let open = !app.ui.collapsed_folders.contains(&id);
    let y = r.min.y + text_y;
    chevron(ui.painter(), pos2(r.min.x + CHEVRON_X, y + 2.0), HEADING_CHEVRON, open, if resp.hovered() { t.accent_text } else { t.text });
    paint_clipped(ui.painter(), pos2(r.min.x + NAME_LEFT, y), (r.width() - NAME_LEFT - 8.0).max(0.0), name, regular(HEADING_SIZE), t.text);
    if resp.clicked() {
        let _ = app.execute("ui.toggleFolder", json!({"id": id}));
    }
    // An account's heading (not the Favorites').
    if id != FAVORITES {
        resp.context_menu(|ui| {
            ui.set_min_width(190.0);
            if ui.add(egui::Button::new(tl!("Properties…")).min_size(vec2(180.0, 0.0))).clicked() {
                let _ = app.run("folder.properties", json!({"account": id}));
                ui.close();
            }
        });
    }
    open
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
    let row = highlight(r);
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
    let y = r.center().y - TEXT_RISE;
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
    let (id, custom, favorite, kind) = (f.id, f.kind == FolderKind::Custom, f.favorite, f.kind);
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
        item(ui, app, "New Folder…", "ui.dialog", json!({"name": "folder", "mode": "new", "parent": id}), true);
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
    paint_clipped(ui.painter(), pos2(r.min.x + NAME_LEFT, r.center().y - TEXT_RISE), width, tl!(step.label()), regular(NAME_SIZE), t.accent_text);
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
    let y = r.center().y - TEXT_RISE;
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
        sheet(ui.painter(), ui.max_rect(), &t);
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
            // it was picked in its account's list and shows there too.
            let current = app.session.view.folder;
            let among_favorites = !folded.contains(&FAVORITES) && favorites.iter().any(|f| f.id == current);
            let in_a_list = lists.iter().any(|(_, _, rows)| rows.iter().any(|(f, _, _)| f.id == current));
            let favorite_selected = among_favorites && !(in_a_list && picked_in_list(ui));
            // The first heading sits right under the top of the pane; each later one starts a
            // new block, with a rule above it.
            let mut first = true;
            let mut head = |app: &mut MailApp, ui: &mut Ui, id: Id, name: &str| {
                if std::mem::take(&mut first) {
                    heading(app, ui, id, name, ROW_H, 7.5, false)
                } else {
                    heading(app, ui, id, name, 2.0 * ROW_H, 34.5, true)
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
            for (account, name, rows) in &lists {
                if !head(app, ui, *account, if name.is_empty() { tl!("Mailbox") } else { name }) {
                    continue;
                }
                for (f, depth, has_children) in rows {
                    folder_row(app, ui, f, *depth, *has_children, "tree", count(f), !favorite_selected && f.id == current);
                }
            }
            ui.add_space(8.0);
        });
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
mod tests;
