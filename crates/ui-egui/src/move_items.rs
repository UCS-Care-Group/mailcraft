// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Move Items and Copy Items (#1176): where the selected messages go when Move to Folder or Copy to
//! Folder runs without a folder (Ctrl+Shift+V and Ctrl+Shift+Y, the menu bar's items, Search
//! Commands). The folders listed are those Home › Move lists: the account in view's, as a tree in
//! the same order. Typing narrows the list to the folders whose place matches and marks the first;
//! the arrow keys, Enter, a click and a double-click choose, and OK runs `mail.move` or `mail.copy`
//! with the folder chosen, for the messages selected then.
//!
//! The dialog moves the selection and nothing else: it takes no ids, so that whoever opens it (an
//! agent may, through `ui.dialog`) can't have the owner move messages they didn't select.

use egui::{Key, Ui, vec2};
use mailcraft_model::{Id, Mailbox};
use serde_json::{Value, json};

use crate::MailApp;
use crate::theme::{Tokens, regular};

/// Most characters of the search box.
const QUERY_MAX: usize = 120;
/// Most rows listed, whatever the mailbox holds.
const ROWS_MAX: usize = 5_000;
/// How deep the tree is walked.
const DEPTH_MAX: usize = 32;

/// The dialog's state.
pub struct MoveItems {
    /// Copy Items, not Move Items.
    pub copy: bool,
    /// The account whose folders are listed.
    account: Id,
    query: String,
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
    depth: usize,
}

/// What the dialog did in a frame.
pub enum Outcome {
    Open,
    Close,
}

impl MoveItems {
    /// Move Items (`copy` false) or Copy Items for the messages selected; `None` when none is, or
    /// there is no account to list.
    pub fn open(app: &MailApp, copy: bool) -> Option<MoveItems> {
        if app.session.sel.messages.is_empty() {
            return None;
        }
        // The account Home › Move lists: the folder in view's, else the default one.
        let mb = &app.session.mailbox;
        let account = mb.folder(app.session.view.folder).map(|f| f.account).or(mb.default_account().map(|a| a.id))?;
        Some(MoveItems { copy, account, query: String::new(), marked: None, error: String::new() })
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
        match app.execute(id, params) {
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
        let rows = rows(&app.session.mailbox, self.account);
        ui.label(if self.copy { tl!("Copy the selected items to the folder:") } else { tl!("Move the selected items to the folder:") });
        let resp =
            ui.add(egui::TextEdit::singleline(&mut self.query).hint_text(tl!("Search folders")).char_limit(QUERY_MAX).desired_width(f32::INFINITY));
        let resp = crate::text_menu::field(resp);
        crate::text_menu::hold(&resp);
        let wanted = mailcraft_model::words::fold(&self.query);
        let shown: Vec<&Row> = rows.iter().filter(|row| wanted.is_empty() || mailcraft_model::words::fold(&row.path).contains(&wanted)).collect();
        let choosable: Vec<Id> = shown.iter().map(|row| row.folder).filter(|f| Some(*f) != here).collect();
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
        egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 1.0);
            for row in &shown {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), egui::Sense::click());
                let can = Some(row.folder) != here;
                if self.marked == Some(row.folder) {
                    ui.painter().rect_filled(r, 4.0, t.selection);
                    if down || up {
                        ui.scroll_to_rect(r, None);
                    }
                } else if can && resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, t.hover);
                }
                let (text, x) =
                    if wanted.is_empty() { (&row.name, r.min.x + 10.0 + 16.0 * row.depth.min(12) as f32) } else { (&row.path, r.min.x + 10.0) };
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

/// The folders of `account`, as a tree in the order Home › Move lists them.
fn rows(mb: &Mailbox, account: Id) -> Vec<Row> {
    fn walk(mb: &Mailbox, account: Id, parent: Option<Id>, depth: usize, out: &mut Vec<Row>) {
        if depth > DEPTH_MAX {
            return;
        }
        for f in mb.children(account, parent) {
            if out.len() >= ROWS_MAX {
                return;
            }
            out.push(Row { folder: f.id, name: crate::nav::folder_name(f).to_string(), path: crate::nav::folder_path(mb, f.id), depth });
            walk(mb, account, Some(f.id), depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(mb, account, None, 0, &mut out);
    out
}
