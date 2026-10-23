// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Getting around: the module bar (Mail, Calendar, People, Tasks) down the left edge, and the
//! folder pane beside it in Mail.

use std::collections::HashMap;

use egui::{Align2, Color32, CornerRadius, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use mailcraft_engine::Module;
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

/// The rank of the folders that lead an account's list, in the order they lead it.
fn lead_rank(kind: FolderKind) -> usize {
    const LEAD: [FolderKind; 4] = [FolderKind::Inbox, FolderKind::Drafts, FolderKind::Sent, FolderKind::Deleted];
    LEAD.iter().position(|k| *k == kind).unwrap_or(LEAD.len())
}

/// Put folders that share a parent (`top`: none, they are an account's top level) in the order
/// the pane lists them: at the top level Inbox, Drafts, Sent Items and Deleted Items lead; every
/// other folder, Archive, Junk Email and Outbox among them, follows by name without regard to
/// case. Subfolders are by name alone.
fn sort_as_listed(list: &mut [&Folder], top: bool) {
    // The id last: two folders of one name keep their places from frame to frame.
    list.sort_by_cached_key(|f| (if top { lead_rank(f.kind) } else { 0 }, f.name.to_lowercase(), f.id));
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

    fn text(self) -> String {
        match self {
            Count::Nothing => String::new(),
            Count::Unread(n) => n.to_string(),
            Count::Total(n) => format!("[{n}]"),
        }
    }
}

/// How many messages each folder holds and how many of those are unread, in one pass over the mail.
fn tallies(mailbox: &Mailbox) -> HashMap<Id, (usize, usize)> {
    let mut out: HashMap<Id, (usize, usize)> = HashMap::new();
    for m in mailbox.messages.values() {
        let entry = out.entry(m.folder).or_default();
        entry.0 = entry.0.saturating_add(1);
        if !m.read {
            entry.1 = entry.1.saturating_add(1);
        }
    }
    out
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
    let resp = ui.interact(r, ui.id().with(("heading", id)), Sense::click());
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
    open
}

/// Where the folder in view was last picked: `true` in its account's list, `false` under
/// Favorites (and before any pick). A folder listed in both is highlighted in that one place.
fn picked_in_list(ui: &Ui) -> bool {
    ui.data(|d| d.get_temp::<bool>(egui::Id::new("folder_picked_in_list"))).unwrap_or(false)
}

/// A folder's row, `selected` when it is the one the folder in view is highlighted in. `key` tells
/// the Favorites' rows (`fav`) from the accounts' (`tree`).
fn folder_row(app: &mut MailApp, ui: &mut Ui, f: &Folder, depth: usize, has_children: bool, key: &str, count: Count, selected: bool) {
    // Under Favorites a shared mailbox's folder says whose it is (#70), and so does one whose name
    // another account's folder has too (two Inboxes); in its tree, its name.
    let name = if key == "fav" { result_folder_label(&app.session.mailbox, f) } else { folder_name(f).to_string() };
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let resp = ui.interact(r, ui.id().with((key, f.id)), Sense::click());
    app.hit.add("folder", f.id, r);
    let row = highlight(r);
    if selected {
        ui.painter().rect_filled(row, 0.0, t.selection);
    } else if resp.hovered() {
        // Quieter than on the ribbon: the pane is darker, and the selected row must stay the one that stands out.
        ui.painter().rect_filled(row, 0.0, t.panel.lerp_to_gamma(t.hover, 0.7));
    }
    let x = r.min.x + depth.min(MAX_INDENT_DEPTH) as f32 * INDENT;
    // The chevron has its own click target, only on a folder with something under it.
    if has_children {
        let tr = Rect::from_min_max(pos2(x + HIGHLIGHT_LEFT, r.min.y), pos2(x + NAME_LEFT, r.max.y));
        let tresp = ui.interact(tr, ui.id().with((key, "twisty", f.id)), Sense::click());
        let open = !app.ui.collapsed_folders.contains(&f.id);
        chevron(ui.painter(), pos2(x + CHEVRON_X, r.center().y), ROW_CHEVRON, open, if tresp.hovered() { t.accent_text } else { t.text });
        if tresp.clicked() {
            let _ = app.execute("ui.toggleFolder", json!({"id": f.id}));
        }
    }
    let y = r.center().y - TEXT_RISE;
    // The count, flush right: unread in bold, a total in brackets, both in the accent.
    let mut name_end = r.max.x - COUNT_RIGHT;
    if count != Count::Nothing {
        let font = if matches!(count, Count::Unread(_)) { bold(NAME_SIZE) } else { regular(NAME_SIZE) };
        let shown = ui.painter().text(pos2(r.max.x - COUNT_RIGHT, y), Align2::RIGHT_CENTER, count.text(), font, t.accent);
        name_end = shown.min.x - 8.0;
    }
    let font = if selected { bold(NAME_SIZE) } else { regular(NAME_SIZE) };
    paint_clipped(ui.painter(), pos2(x + NAME_LEFT, y), (name_end - x - NAME_LEFT).max(0.0), &name, font, t.text);
    if resp.clicked() {
        ui.data_mut(|d| d.insert_temp(egui::Id::new("folder_picked_in_list"), key == "tree"));
        let _ = app.run("folder.select", json!({"id": f.id}));
    }
    // Messages dragged out of the list are filed in the folder they are let go over.
    if resp.dnd_hover_payload::<crate::mail_view::MessageDrag>().is_some() {
        ui.painter().rect_stroke(row, 0.0, Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
    }
    if let Some(drag) = resp.dnd_release_payload::<crate::mail_view::MessageDrag>() {
        // Only what isn't here already.
        let ids: Vec<Id> = drag.0.iter().copied().filter(|m| app.session.mailbox.message(*m).is_some_and(|m| m.folder != f.id)).collect();
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
        item(ui, app, "Rename Folder…", "ui.dialog", json!({"name": "folder", "mode": "rename", "id": id}), custom);
        item(ui, app, "Delete Folder", "folder.delete", json!({"id": id}), custom);
        ui.separator();
        item(ui, app, "Mark All as Read", "mail.markAllRead", json!({"folder": id}), true);
        item(ui, app, "Run Rules Now", "rules.run", json!({"folder": id}), true);
        if matches!(kind, FolderKind::Deleted | FolderKind::Junk) {
            item(ui, app, "Empty Folder", "folder.empty", json!({"id": id}), true);
        }
        ui.separator();
        if from_outlook {
            item(ui, app, "Favorites Are Set in Outlook", "folder.favorite", json!({"id": id}), false);
        } else {
            item(ui, app, if favorite { "Remove from Favorites" } else { "Add to Favorites" }, "folder.favorite", json!({"id": id}), true);
        }
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
    let resp = ui.interact(highlight(r), ui.id().with(("favorites_step", account, step.code())), Sense::click());
    let kind = match step {
        crate::favorites_sync::Step::CopyMessage => "favoritesCopyMessage",
        crate::favorites_sync::Step::Help => "favoritesHelp",
    };
    app.hit.add(kind, account, r);
    if resp.hovered() {
        ui.painter().rect_filled(highlight(r), 0.0, t.panel.lerp_to_gamma(t.hover, 0.7));
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
        ui.interact(r, ui.id().with(("favorites_note", account)), Sense::hover()).on_hover_text(about);
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
            let resp = ui.interact(highlight(r), ui.id().with(("favorites_offer", account)), Sense::click());
            app.hit.add("favoritesOffer", account, r);
            if resp.hovered() {
                ui.painter().rect_filled(highlight(r), 0.0, t.panel.lerp_to_gamma(t.hover, 0.7));
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
            let resp = ui.interact(highlight(r), ui.id().with(("favorites_note", account)), Sense::hover());
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
            let resp = ui.interact(link, ui.id().with(("favorites_retry", account)), Sense::click());
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
    let resp = ui.interact(button, ui.id().with("hide_pane"), Sense::click());
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
        let resp = ui.interact(button, ui.id().with("show_pane"), Sense::click());
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
        .show_separator_line(!t.dark)
        .frame(egui::Frame::NONE.fill(t.canvas));
    let shown = panel.show(ui, |ui| {
        sheet(ui.painter(), ui.max_rect(), &t);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        pane_top(app, ui, &t);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let counts = tallies(&app.session.mailbox);
            let count = |f: &Folder| {
                let (total, unread) = counts.get(&f.id).copied().unwrap_or_default();
                Count::of(f.kind, total, unread)
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
mod tests {
    use mailcraft_engine::Session;

    use super::*;
    use crate::Services;

    /// Wednesday 2026-10-14 16:00 UTC.
    const NOW: i64 = 1_791_993_600;

    fn names(folders: &[&Folder]) -> Vec<String> {
        folders.iter().map(|f| f.name.clone()).collect()
    }

    /// A standard folder still called by its default name, or the server's folder of its kind,
    /// is named in the interface language; one the owner named keeps its name; English shows
    /// every folder by its own name. What is kept never changes.
    #[test]
    fn folders_are_named_in_the_interface_language_by_their_kind() {
        let folder = |name: &str, kind: FolderKind, remote: &str| Folder { name: name.into(), kind, remote: remote.into(), ..Default::default() };
        assert_eq!(named_by(&folder("Inbox", FolderKind::Inbox, "")), Some(Named::Kind(FolderKind::Inbox)));
        assert_eq!(named_by(&folder("Sent Items", FolderKind::Sent, "")), Some(Named::Kind(FolderKind::Sent)));
        assert_eq!(named_by(&folder("Old mail", FolderKind::Archive, "")), None);
        assert_eq!(named_by(&folder("Sent Mail", FolderKind::Sent, "[Gmail]/Sent Mail")), Some(Named::Kind(FolderKind::Sent)));
        assert_eq!(named_by(&folder(mailcraft_model::IMPORTED, FolderKind::Custom, "")), Some(Named::Imported));
        assert_eq!(named_by(&folder("Imported", FolderKind::Custom, "Imported")), None);
        assert_eq!(named_by(&folder("Projects", FolderKind::Custom, "")), None);
        assert_eq!(mailcraft_model::IMPORTED, "Imported", "the text `folder_name` translates");
        // (The tests' window is in English: every folder by its own name.)
        assert_eq!(folder_name(&folder("Sent Mail", FolderKind::Sent, "[Gmail]/Sent Mail")), "Sent Mail");
        // A tangled tree still ends.
        let mut mb = Mailbox::default();
        mb.folders.push(Folder { id: 1, parent: Some(2), ..folder("a", FolderKind::Custom, "") });
        mb.folders.push(Folder { id: 2, parent: Some(1), ..folder("b", FolderKind::Custom, "") });
        assert!(folder_path(&mb, 1).split('/').count() <= 33);
    }

    /// The folders of `account` under `parent` (`None`: its top level), in the pane's order.
    fn ordered(mailbox: &Mailbox, account: Id, parent: Option<Id>) -> Vec<&Folder> {
        let mut list: Vec<&Folder> = mailbox.folders.iter().filter(|f| f.account == account && f.parent == parent).collect();
        sort_as_listed(&mut list, parent.is_none());
        list
    }

    /// Inbox, Drafts, Sent Items and Deleted Items lead; everything else, the other standard
    /// folders among it, follows by name whatever its case; subfolders are by name alone.
    #[test]
    fn folders_are_listed_leading_four_then_by_name() {
        let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@lumen.example");
        let account = mb.accounts.first().unwrap().id;
        let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
        for name in ["Zebra", "apple", "Banana", "outings", "Projects", "receipts"] {
            mb.add_folder(account, None, name).unwrap();
        }
        for name in ["beta", "Alpha", "delta", "Charlie"] {
            mb.add_folder(account, Some(inbox), name).unwrap();
        }
        assert_eq!(
            names(&ordered(&mb, account, None)),
            [
                "Inbox",
                "Drafts",
                "Sent Items",
                "Deleted Items",
                "apple",
                "Archive",
                "Banana",
                "Junk Email",
                "Outbox",
                "outings",
                "Projects",
                "receipts",
                "Zebra"
            ]
        );
        assert_eq!(names(&ordered(&mb, account, Some(inbox))), ["Alpha", "beta", "Charlie", "delta"]);

        // Depth first, each level in that order, and nothing from under a folded folder.
        let charlie = mb.folders.iter().find(|f| f.name == "Charlie").unwrap().id;
        mb.add_folder(account, Some(charlie), "two").unwrap();
        mb.add_folder(account, Some(charlie), "One").unwrap();
        let listed = |folded: &[Id]| {
            visible(&mb, folded, account).iter().map(|(f, depth, has_children)| (f.name.clone(), *depth, *has_children)).collect::<Vec<_>>()
        };
        let open = listed(&[]);
        let head: Vec<(&str, usize, bool)> = open.iter().take(8).map(|(n, d, c)| (n.as_str(), *d, *c)).collect();
        assert_eq!(
            head,
            [
                ("Inbox", 0, true),
                ("Alpha", 1, false),
                ("beta", 1, false),
                ("Charlie", 1, true),
                ("One", 2, false),
                ("two", 2, false),
                ("delta", 1, false),
                ("Drafts", 0, false)
            ]
        );
        assert_eq!(listed(&[charlie]).len(), open.len() - 2);
        assert_eq!(listed(&[inbox]).iter().filter(|(_, depth, _)| *depth > 0).count(), 0);

        // Another account's folders are its own, in the same order.
        let other = mb.add_account(mailcraft_model::Account { name: "studio".into(), ..Default::default() });
        mb.add_folder(other, None, "aardvark").unwrap();
        assert_eq!(
            names(&ordered(&mb, other, None)).iter().take(5).collect::<Vec<_>>(),
            ["Inbox", "Drafts", "Sent Items", "Deleted Items", "aardvark"]
        );
        assert_eq!(ordered(&mb, account, None).len(), 13);
    }

    /// A mailbox may come from anywhere: folders nested without end, in a ring, or by the thousand
    /// are listed up to a point and no further.
    #[test]
    fn a_hostile_folder_tree_is_listed_up_to_a_point() {
        let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@lumen.example");
        let account = mb.accounts.first().unwrap().id;
        let mut parent = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
        for i in 0..3 * MAX_DEPTH {
            let id = mb.add_folder(account, None, &format!("level {i}")).unwrap();
            mb.folder_mut(id).unwrap().parent = Some(parent);
            parent = id;
        }
        let rows = visible(&mb, &[], account);
        assert_eq!(rows.iter().map(|(_, depth, _)| *depth).max(), Some(MAX_DEPTH));

        // Two folders that are each other's parent hang from nothing: neither is listed.
        let a = mb.add_folder(account, None, "ring a").unwrap();
        let b = mb.add_folder(account, None, "ring b").unwrap();
        mb.folder_mut(a).unwrap().parent = Some(b);
        mb.folder_mut(b).unwrap().parent = Some(a);
        let me = mb.add_folder(account, None, "ring of one").unwrap();
        mb.folder_mut(me).unwrap().parent = Some(me);
        assert!(!visible(&mb, &[], account).iter().any(|(f, _, _)| f.name.starts_with("ring")));

        for i in 0..MAX_ROWS + 50 {
            mb.folders.push(Folder { id: 1_000_000 + i as Id, account, name: format!("f{i}"), ..Default::default() });
        }
        assert_eq!(visible(&mb, &[], account).len(), MAX_ROWS);
    }

    /// Unread mail is a bold number; Drafts, Outbox and Junk Email show all they hold, in square
    /// brackets; sent and deleted mail shows nothing.
    #[test]
    fn counts_are_unread_or_a_total_in_brackets() {
        assert_eq!(Count::of(FolderKind::Inbox, 9, 4), Count::Unread(4));
        assert_eq!(Count::of(FolderKind::Custom, 9, 4).text(), "4");
        assert_eq!(Count::of(FolderKind::Archive, 9, 0), Count::Nothing);
        assert_eq!(Count::of(FolderKind::Drafts, 3, 0), Count::Total(3));
        assert_eq!(Count::of(FolderKind::Junk, 28, 5).text(), "[28]");
        assert_eq!(Count::of(FolderKind::Outbox, 1, 1).text(), "[1]");
        assert_eq!(Count::of(FolderKind::Drafts, 0, 0).text(), "");
        assert_eq!(Count::of(FolderKind::Sent, 5, 5), Count::Nothing);
        assert_eq!(Count::of(FolderKind::Deleted, 5, 5), Count::Nothing);

        // The tallies behind them, against the mailbox's own counting.
        let mb = mailcraft_engine::sample::mailbox(NOW, 0);
        let counts = tallies(&mb);
        assert!(mb.folders.iter().any(|f| mb.unread_count(f.id) > 0));
        for f in &mb.folders {
            assert_eq!(counts.get(&f.id).copied().unwrap_or_default(), (mb.count(f.id), mb.unread_count(f.id)), "{}", f.name);
        }
    }

    fn window() -> egui_kittest::Harness<'static, MailApp> {
        let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
        app.autosave = false;
        let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
        harness.run_steps(4);
        // The sample has reminders due, and their window would lie over the top of the pane.
        let _ = harness.state_mut().execute("ui.dismissReminder", serde_json::json!({"all": true}));
        harness.run_steps(3);
        harness
    }

    fn click(harness: &mut egui_kittest::Harness<'static, MailApp>, pos: Pos2) {
        harness.event(egui::Event::PointerMoved(pos));
        harness.step();
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
            harness.step();
        }
        harness.run_steps(2);
    }

    /// Two accounts' Inboxes among the Favorites say which account each is, as in a search of every
    /// mailbox; under each account's own heading they stay plain Inbox.
    #[test]
    fn two_accounts_inboxes_among_the_favorites_are_told_apart() {
        let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
        app.autosave = false;
        let mb = &mut app.session.mailbox;
        let first = mb.inbox().unwrap();
        let work = mb.add_account(mailcraft_model::Account { name: "Work".into(), email: "me@work.example".into(), ..Default::default() });
        let second = mb.folder_of_kind(work, FolderKind::Inbox).unwrap();
        for id in [first, second] {
            app.execute("folder.favorite", serde_json::json!({"id": id, "value": true})).unwrap();
        }
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| folder_pane(&mut app, ui));
        out.textures_delta.clear();
        let texts: Vec<String> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t == "Inbox - Work"), "{texts:?}");
        let mine = app.session.mailbox.accounts.first().unwrap();
        let mine = if mine.name.trim().is_empty() { mine.email.trim() } else { mine.name.trim() };
        assert!(texts.iter().any(|t| *t == format!("Inbox - {mine}")), "{texts:?}");
        assert!(texts.iter().any(|t| t == "Inbox"), "each account's own tree keeps the plain name: {texts:?}");
    }

    /// The pane lists the folders in order down the screen, one row each, further in at each level.
    #[test]
    fn rows_go_down_the_pane_in_order() {
        let harness = window();
        let app = harness.state();
        let account = app.session.mailbox.accounts.first().unwrap().id;
        let rows = visible(&app.session.mailbox, &[], account);
        assert!(rows.iter().any(|(_, depth, _)| *depth == 2), "the sample has folders two levels down");
        // A favourite's first row is the one under Favorites: look at the others.
        let listed: Vec<Rect> = rows.iter().filter(|(f, _, _)| !f.favorite).map(|(f, _, _)| app.hit.rect("folder", f.id).unwrap()).collect();
        assert!(listed.len() > 5);
        for pair in listed.windows(2) {
            assert!(pair[1].min.y >= pair[0].max.y - 0.5, "{pair:?}");
            assert!((pair[0].height() - ROW_H).abs() < 0.01, "{pair:?}");
        }
        // Favourites are above the account's heading, the account's own rows under it.
        let heading = app.hit.rect("folderHeading", account).unwrap();
        let favorites = app.hit.rect("folderHeading", FAVORITES).unwrap();
        assert!(favorites.max.y <= heading.min.y);
        assert!(listed.iter().all(|r| r.min.y >= heading.max.y - 0.5));
        let favorite = app.session.mailbox.folders.iter().find(|f| f.favorite).unwrap().id;
        let row = app.hit.rect("folder", favorite).unwrap();
        assert!(row.min.y >= favorites.max.y - 0.5 && row.max.y <= heading.min.y + 0.5);
    }

    /// A click on the Favorites heading or on an account's folds what is under it away, another
    /// brings it back, and the choice is saved with the settings.
    #[test]
    fn headings_fold_and_the_settings_remember() {
        let mut harness = window();
        let account = harness.state().session.mailbox.accounts.first().unwrap().id;
        let drafts = harness.state().session.mailbox.folder_of_kind(account, FolderKind::Drafts).unwrap();
        let at = |h: &egui_kittest::Harness<'static, MailApp>, kind: &str, id: Id| h.state().hit.rect(kind, id);
        assert!(at(&harness, "folder", drafts).is_some());
        let before = at(&harness, "folderHeading", account).unwrap();

        // Favorites folded: the account's heading moves up to where the favourites were.
        let favorites = at(&harness, "folderHeading", FAVORITES).unwrap();
        click(&mut harness, favorites.center());
        assert!(harness.state().ui.collapsed_folders.contains(&FAVORITES));
        let after = at(&harness, "folderHeading", account).unwrap();
        assert!(after.min.y < before.min.y - ROW_H, "{before:?} -> {after:?}");
        assert!(at(&harness, "folder", drafts).is_some(), "the account's folders stay");

        // The account folded: none of its folders is listed.
        click(&mut harness, after.center());
        assert!(harness.state().ui.collapsed_folders.contains(&account));
        assert!(harness.state().session.mailbox.folders.iter().all(|f| at(&harness, "folder", f.id).is_none()));

        // Saved, and restored into another window.
        let saved = serde_json::to_string(&harness.state().prefs()).unwrap();
        let restored: crate::UiState = serde_json::from_str(&saved).unwrap();
        assert!(restored.collapsed_folders.contains(&FAVORITES) && restored.collapsed_folders.contains(&account));

        // And back.
        let heading = at(&harness, "folderHeading", account).unwrap();
        click(&mut harness, heading.center());
        assert!(!harness.state().ui.collapsed_folders.contains(&account));
        assert!(at(&harness, "folder", drafts).is_some());
        let favorites = at(&harness, "folderHeading", FAVORITES).unwrap();
        click(&mut harness, favorites.center());
        assert!(harness.state().ui.collapsed_folders.is_empty());
    }

    /// The button at the top of the pane puts it away; a narrow strip stays, with the button that
    /// brings it back.
    #[test]
    fn the_pane_hides_and_its_strip_brings_it_back() {
        let mut harness = window();
        let inbox = harness.state().session.view.folder;
        let shown = |h: &egui_kittest::Harness<'static, MailApp>| h.state().session.view.folder_pane;
        assert!(shown(&harness) && harness.state().hit.rect("folderPaneShow", 0).is_none());
        let wide = harness.state().hit.rect("folderHeading", FAVORITES).unwrap().width();
        let hide = harness.state().hit.rect("folderPaneHide", 0).unwrap();
        click(&mut harness, hide.center());
        assert!(!shown(&harness));
        assert!(harness.state().hit.rect("folder", inbox).is_none() && harness.state().hit.rect("folderPaneHide", 0).is_none());
        let show = harness.state().hit.rect("folderPaneShow", 0).unwrap();
        assert!(show.max.x - hide.min.x < 0.0 && show.width() < wide, "the strip is at the pane's left edge, and narrow: {show:?}");
        assert!((show.center().y - hide.center().y).abs() < 0.5, "where the other button was");
        click(&mut harness, show.center());
        assert!(shown(&harness) && harness.state().hit.rect("folder", inbox).is_some());
    }

    /// A click on a folder's row shows that folder; a click on its chevron folds its subfolders
    /// away and leaves the folder in view alone.
    #[test]
    fn a_row_selects_and_its_chevron_folds() {
        let mut harness = window();
        let account = harness.state().session.mailbox.accounts.first().unwrap().id;
        let (inbox, drafts) = {
            let mb = &harness.state().session.mailbox;
            (mb.folder_of_kind(account, FolderKind::Inbox).unwrap(), mb.folder_of_kind(account, FolderKind::Drafts).unwrap())
        };
        let child = harness.state().session.mailbox.folders.iter().find(|f| f.parent == Some(inbox) && !f.favorite).unwrap().id;
        let row = harness.state().hit.rect("folder", drafts).unwrap();
        click(&mut harness, row.center());
        assert_eq!(harness.state().session.view.folder, drafts);

        // The Inbox is a favourite too: its row in the account's list is the one with the chevron.
        let heading = harness.state().hit.rect("folderHeading", account).unwrap();
        let child_row = harness.state().hit.rect("folder", child).unwrap();
        let inbox_row = Rect::from_min_size(pos2(child_row.min.x, heading.max.y), vec2(child_row.width(), ROW_H));
        click(&mut harness, pos2(inbox_row.min.x + CHEVRON_X, inbox_row.center().y));
        assert!(harness.state().ui.collapsed_folders.contains(&inbox));
        assert_eq!(harness.state().session.view.folder, drafts, "the chevron doesn't select");
        assert!(harness.state().hit.rect("folder", child).is_none());
        click(&mut harness, pos2(inbox_row.min.x + CHEVRON_X, inbox_row.center().y));
        assert!(harness.state().hit.rect("folder", child).is_some());
    }
}
