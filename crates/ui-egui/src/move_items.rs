// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Move Items and Copy Items (#1176): where the selected messages go when Move to Folder or Copy to
//! Folder runs without a folder (Ctrl+Shift+V and Ctrl+Shift+Y, the menu bar's items, Search
//! Commands). The folders listed are those of every mailbox the owner has added: the
//! account in view's first, then the others as the folder pane has them, each as a tree in the
//! same order under the account's name, since a move reaches another account's server too (#451).
//! Typing narrows the list to the folders whose place or account matches and marks the first; the
//! arrow keys, Enter, a click and a double-click choose, and OK runs `mail.move` or `mail.copy`
//! with the folder chosen, for the messages selected then, as the ribbon's Move menu runs them
//! (`MailApp::run`: what the window says after a move is said here too). The folders are listed
//! as they were when the dialog opened; a search looks through all of them, and as many rows as
//! the list shows at most are drawn.
//!
//! The dialog moves the selection and nothing else: it takes no ids, so that whoever opens it (an
//! agent may, through `ui.dialog`) can't have the owner move messages they didn't select.

use egui::{Key, Ui, vec2};
use std::collections::HashMap;

use mailcraft_model::{Account, Folder, FolderKind, Id, Mailbox};
use serde_json::{Value, json};

use crate::MailApp;
use crate::theme::{Tokens, regular};

/// Most characters of the search box.
const QUERY_MAX: usize = 120;
/// Most rows drawn, whatever the mailbox holds.
const ROWS_MAX: usize = 5_000;
/// Most folders read for the list and its search.
const FOLDERS_MAX: usize = 100_000;
/// How deep the tree is walked.
const DEPTH_MAX: usize = 32;

/// The dialog's state.
pub struct MoveItems {
    /// Copy Items, not Move Items.
    pub copy: bool,
    query: String,
    /// Every account's folders, as trees, read once when the dialog opened.
    rows: Vec<Row>,
    /// The folder the keys or a click marked.
    marked: Option<Id>,
    error: String,
}

/// One folder of the list.
struct Row {
    folder: Id,
    name: String,
    /// Its place in its account, shown for a search's matches.
    path: String,
    /// The account it belongs to, as a folder row names it (`Name (address)`), so that two
    /// Inboxes are told apart.
    account: String,
    /// The place and the account folded as a search folds them, worked out once.
    fold: String,
    depth: usize,
    /// The account's name, on the first of its folders: the heading above its tree.
    heading: Option<String>,
}

/// What the dialog did in a frame.
pub enum Outcome {
    Open,
    Close,
}

impl MoveItems {
    /// Move Items (`copy` false) or Copy Items for the messages selected; `None` when none is.
    pub fn open(app: &MailApp, copy: bool) -> Option<MoveItems> {
        if app.session.sel.messages.is_empty() {
            return None;
        }
        // (The mailbox in view's folders first, then those of every other account: a message
        // reaches another account too.)
        let mb = &app.session.mailbox;
        let in_view = mb.folder(app.session.view.folder).map(|f| f.account).or_else(|| mb.default_account().map(|a| a.id));
        let rows = rows(mb, in_view);
        Some(MoveItems { copy, query: String::new(), rows, marked: None, error: String::new() })
    }

    /// The dialog's name, as `ui.dialog` takes it.
    pub fn name(&self) -> &'static str {
        if self.copy { "copyItems" } else { "moveItems" }
    }

    /// The window's title, in the interface language.
    pub fn title(&self) -> &'static str {
        if self.copy { tl!("Copy Items") } else { tl!("Move Items") }
    }

    /// The folder marked now, for tests.
    #[cfg(test)]
    pub fn marked(&self) -> Option<Id> {
        self.marked
    }

    /// The folders listed, in order: each with its depth and its place, for tests.
    #[cfg(test)]
    pub fn listed(&self) -> Vec<(Id, usize, String)> {
        self.rows.iter().map(|r| (r.folder, r.depth, r.path.clone())).collect()
    }

    /// Marks `folder`, as a click on its row does.
    #[cfg(test)]
    pub fn mark(&mut self, folder: Id) {
        self.marked = Some(folder);
    }

    /// The command OK runs with the folder marked; `None` while none is.
    pub fn command(&self) -> Option<(&'static str, Value)> {
        let folder = self.marked?;
        Some((if self.copy { "mail.copy" } else { "mail.move" }, json!({"folder": folder})))
    }

    /// Runs the command with the folder marked: the dialog closes when it is done, and says why when
    /// it isn't.
    pub fn go(&mut self, app: &mut MailApp) -> Outcome {
        let Some((id, params)) = self.command() else { return Outcome::Open };
        match app.run(id, params) {
            Ok(_) => Outcome::Close,
            Err(e) => {
                self.error = e;
                Outcome::Open
            }
        }
    }

    /// The dialog's body: the search box, the list and OK and Cancel. `enter` is the frame's Enter.
    pub fn ui(&mut self, ui: &mut Ui, app: &mut MailApp, t: &Tokens, enter: bool) -> Outcome {
        // (A move into the folder in view moves nothing: it isn't offered, as in Home › Move.)
        let here = (!self.copy).then_some(app.session.view.folder);
        ui.label(if self.copy { tl!("Copy the selected items to the folder:") } else { tl!("Move the selected items to the folder:") });
        let resp =
            ui.add(egui::TextEdit::singleline(&mut self.query).hint_text(tl!("Search folders")).char_limit(QUERY_MAX).desired_width(f32::INFINITY));
        let resp = crate::text_menu::field(resp);
        crate::text_menu::hold(&resp);
        let wanted = mailcraft_model::words::fold(&self.query);
        // (Every folder is searched; the rows drawn stop at the most a list shows.)
        let shown: Vec<usize> = (0..self.rows.len()).filter(|i| wanted.is_empty() || self.rows[*i].fold.contains(&wanted)).take(ROWS_MAX).collect();
        let choosable: Vec<Id> = shown.iter().map(|i| self.rows[*i].folder).filter(|f| Some(*f) != here).collect();
        // A search marks its first match; a folder that is no longer listed isn't marked.
        if resp.changed() {
            self.marked = if wanted.is_empty() { None } else { choosable.first().copied() };
        }
        if self.marked.is_some_and(|m| !choosable.contains(&m)) {
            self.marked = None;
        }
        let (down, up) = ui.input(|i| (i.key_pressed(Key::ArrowDown), i.key_pressed(Key::ArrowUp)));
        if (down || up) && !choosable.is_empty() {
            let last = choosable.len() - 1;
            let at = self.marked.and_then(|m| choosable.iter().position(|f| *f == m));
            let next = match (at, down) {
                (None, true) => 0,
                (None, false) => last,
                (Some(i), true) => {
                    if i >= last {
                        0
                    } else {
                        i + 1
                    }
                }
                (Some(i), false) => i.checked_sub(1).unwrap_or(last),
            };
            self.marked = choosable.get(next).copied();
        }
        let mut chosen = false;
        // (Two mailboxes can each have an Inbox: then a folder says whose it is.)
        let several = self.rows.first().is_some_and(|f| self.rows.iter().any(|r| r.account != f.account));
        egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 1.0);
            for &i in &shown {
                let row = &self.rows[i];
                // (Above each account's tree: its name, which a search's matches show instead.
                // With one account there is nothing to tell apart, so no heading.)
                if wanted.is_empty() && several {
                    if let Some(heading) = &row.heading {
                        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), egui::Sense::hover());
                        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, heading));
                        crate::theme::paint_clipped(
                            ui.painter(),
                            egui::pos2(r.min.x + 10.0, r.center().y),
                            (r.max.x - r.min.x - 20.0).max(40.0),
                            heading,
                            crate::theme::bold(12.0),
                            t.text_dim,
                        );
                    }
                }
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), egui::Sense::click());
                let can = Some(row.folder) != here;
                // (Painted: its name, whether it is marked and whether it can be, for a screen reader.
                // The account is named too when the list holds more than one, so that two Inboxes
                // are told apart.)
                let marked = self.marked == Some(row.folder);
                let said = if several { format!("{} - {}", row.path, row.account) } else { row.path.clone() };
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ListBoxOption, can, &said));
                // (A folder is chosen, AccessKit's `selected`, not switched on: as in `gmail_tabs`.)
                ui.ctx().accesskit_node_builder(resp.id, |node| node.set_selected(marked));
                if marked {
                    ui.painter().rect_filled(r, 4.0, t.selection);
                    if down || up {
                        ui.scroll_to_rect(r, None);
                    }
                } else if can && resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, t.hover);
                }
                let (text, x): (&str, f32) = if wanted.is_empty() {
                    (&row.name, r.min.x + 10.0 + 16.0 * row.depth.min(12) as f32)
                } else if several {
                    // (A search's matches name their account: the same place exists in each.)
                    (&said, r.min.x + 10.0)
                } else {
                    (&row.path, r.min.x + 10.0)
                };
                let ink = if can { t.text } else { t.text_disabled };
                crate::theme::paint_clipped(ui.painter(), egui::pos2(x, r.center().y), (r.max.x - x - 10.0).max(40.0), text, regular(13.0), ink);
                if can && resp.clicked() {
                    self.marked = Some(row.folder);
                }
                if can && resp.double_clicked() {
                    self.marked = Some(row.folder);
                    chosen = true;
                }
            }
            if shown.is_empty() {
                ui.label(egui::RichText::new(tl!("No folders match")).color(t.text_dim));
            }
        });
        if !self.error.is_empty() {
            ui.label(egui::RichText::new(self.error.as_str()).color(t.red));
        }
        let (ok, cancel) = crate::dialogs::buttons(ui, tl!("OK"), self.marked.is_some());
        if cancel {
            return Outcome::Close;
        }
        if ok || chosen || (enter && self.marked.is_some()) { self.go(app) } else { Outcome::Open }
    }
}

/// Every account's folders as trees in the order Home › Move lists them ([`Mailbox::children`]'s
/// order), each with its place in its account: the account in view's first, then the others' as
/// the folder pane has them. Built in one pass over the folders, so that a mailbox with
/// very many of them opens the dialog without a wait.
fn rows(mb: &Mailbox, in_view: Option<Id>) -> Vec<Row> {
    let rank = |f: &Folder| FolderKind::STANDARD.iter().position(|k| *k == f.kind).unwrap_or(usize::MAX);
    // (Each account's tree on its own, so that one account's folders are never looked through
    // for another's.)
    let mut under: HashMap<(Id, Option<Id>), Vec<&Folder>> = HashMap::new();
    for f in &mb.folders {
        under.entry((f.account, f.parent)).or_default().push(f);
    }
    for v in under.values_mut() {
        v.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    }
    let mut accounts = mb.accounts_in_order();
    if let Some(at) = accounts.iter().position(|a| Some(a.id) == in_view) {
        let a = accounts.remove(at);
        accounts.insert(0, a);
    }
    let mut out = Vec::new();
    for a in accounts {
        let first = out.len();
        walk(&under, a, None, "", 0, &mut out);
        // (The account's name above the first of its folders, none for an account that has none.)
        if let Some(row) = out.get_mut(first) {
            row.heading = Some(crate::nav::account_label(a));
        }
    }
    out
}

/// `account`'s folders, and those under `parent`, appended to `out` as a tree.
fn walk(under: &HashMap<(Id, Option<Id>), Vec<&Folder>>, account: &Account, parent: Option<Id>, above: &str, depth: usize, out: &mut Vec<Row>) {
    if depth > DEPTH_MAX {
        return;
    }
    for f in under.get(&(account.id, parent)).into_iter().flatten() {
        if out.len() >= FOLDERS_MAX {
            return;
        }
        let name = crate::nav::folder_name(f).to_string();
        let path = if above.is_empty() { name.clone() } else { format!("{above}/{name}") };
        // (A search finds a folder by its place, by the mailbox it is in, or by its address.)
        let label = crate::nav::account_label(account);
        let fold = mailcraft_model::words::fold(&format!("{path} - {label} - {}", account.email.trim()));
        let said = crate::nav::account_name(account);
        out.push(Row { folder: f.id, name, path: path.clone(), account: said, fold, depth, heading: None });
        walk(under, account, Some(f.id), &path, depth + 1, out);
    }
}
