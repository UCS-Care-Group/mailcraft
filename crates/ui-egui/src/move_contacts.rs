// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Move to Contacts Folder… (#1144): which contacts folder the contacts go into (those a contact
//! can be put in, and On this computer), and, when that folder has somebody with one of the same
//! e-mail addresses already, whether to open that one or move anyway. It only chooses: the choice
//! runs `people.moveToFolder`.

use egui::{RichText, Ui};
use mailcraft_engine::cmd::contacts_move::{NAMED, already_in};
use mailcraft_engine::cmd::contacts_remote::home;
use mailcraft_engine::cmd::people::contact_accounts;
use mailcraft_model::contact_sync::{ContactFolder, FolderOwner};
use mailcraft_model::{Id, Mailbox};
use serde_json::{Value, json};

use crate::MailApp;
use crate::dialogs::buttons;

/// The dialog's width.
pub const WIDTH: f32 = 420.0;
/// Most folders listed.
const MAX_ROWS: usize = 200;

#[derive(Debug)]
pub struct MoveContacts {
    ids: Vec<Id>,
    folder: Option<Id>,
    /// The contact already in the chosen folder, once asked about: (that one, the folder).
    duplicate: Option<(Id, Id)>,
    error: String,
}

/// What the dialog calls a contacts folder: its name, with whose it is in brackets (the
/// account's address, or the shared mailbox's); one a person shared goes by its own name.
pub fn folder_label(mailbox: &Mailbox, f: &ContactFolder) -> String {
    let whose = match &f.owner {
        FolderOwner::Own => mailbox.account(f.account).map(|a| a.email.as_str()).unwrap_or_default(),
        FolderOwner::Mailbox(address) => address,
        FolderOwner::Person(_) => "",
    };
    if whose.is_empty() { f.name.clone() } else { format!("{} ({whose})", f.name) }
}

/// Where a contact can be moved: On this computer (0), then each folder of an account that keeps
/// contacts and may be written, by account, its main folder first.
pub fn targets(mailbox: &Mailbox) -> Vec<(Id, String)> {
    let accounts = contact_accounts(mailbox);
    let mut folders: Vec<&ContactFolder> = mailbox.contact_folders.iter().filter(|f| f.writable && accounts.contains(&f.account)).collect();
    folders.sort_by_key(|f| (accounts.iter().position(|a| *a == f.account), !(f.owner.is_own() && f.is_main()), f.name.to_lowercase()));
    std::iter::once((0, tl!("On this computer").to_string()))
        .chain(folders.into_iter().take(MAX_ROWS).map(|f| (f.id, folder_label(mailbox, f))))
        .collect()
}

impl MoveContacts {
    /// For the contacts `params` names (`ids`), else those picked together in the list, else the
    /// selected one; none when there is nobody to move.
    pub fn open(app: &MailApp, params: &Value) -> Option<MoveContacts> {
        let named: Vec<Id> = params.get("ids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
        let ids = if !named.is_empty() {
            named
        } else if app.people_picked.len() >= 2 {
            app.people_picked.clone()
        } else {
            vec![app.session.sel.contact?]
        };
        let ids: Vec<Id> = ids.into_iter().filter(|id| app.session.mailbox.contact(*id).is_some()).collect();
        (!ids.is_empty()).then_some(MoveContacts { ids, folder: None, duplicate: None, error: String::new() })
    }

    /// Draws it; true once the contacts are moved, the one already there is opened, or it is
    /// cancelled.
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> bool {
        let t = crate::theme::Tokens::get(ui.ctx());
        if let Some((there, folder)) = self.duplicate {
            return self.ask(app, ui, there, folder);
        }
        let mailbox = &app.session.mailbox;
        let n = self.ids.len() as u64;
        let name = if n == 1 { self.ids.first().and_then(|id| mailbox.contact(*id)).map(|c| c.name()).unwrap_or_default() } else { String::new() };
        let lead = if n == 1 {
            crate::i18n::fmt(tl!("Move \"{name}\" to:"), &[("name", &name)])
        } else {
            tln!(n, "Move {n} contact to:", "Move {n} contacts to:")
        };
        ui.label(lead);
        // Where they all are already isn't offered.
        let here: Vec<Id> =
            self.ids.iter().filter_map(|id| mailbox.contact(*id)).map(|c| if home(mailbox, c.folder).is_some() { c.folder } else { 0 }).collect();
        let rows: Vec<(Id, String)> = targets(mailbox).into_iter().filter(|(id, _)| !here.iter().all(|h| h == id)).collect();
        if self.folder.is_none_or(|f| !rows.iter().any(|r| r.0 == f)) {
            self.folder = rows.first().map(|r| r.0);
        }
        let mut chosen = None;
        egui::ScrollArea::vertical().max_height(280.0).auto_shrink([false, true]).show(ui, |ui| {
            for (id, label) in &rows {
                // A row as wide as the list, so the whole line takes a click.
                let row = egui::Button::selectable(self.folder == Some(*id), label.as_str()).min_size(egui::vec2(ui.available_width(), 28.0));
                let r = ui.add(row);
                if r.clicked() {
                    self.folder = Some(*id);
                }
                if r.double_clicked() {
                    chosen = Some(*id);
                }
            }
            if rows.is_empty() {
                ui.label(RichText::new(tl!("There is no other contacts folder to move to.")).color(t.text_dim));
            }
        });
        if !self.error.is_empty() {
            ui.label(RichText::new(self.error.as_str()).color(t.red));
        }
        let (ok, cancel) = buttons(ui, tl!("Move"), self.folder.is_some());
        if (ok || enter) && chosen.is_none() {
            chosen = self.folder;
        }
        if let Some(folder) = chosen {
            return self.choose(app, folder);
        }
        cancel
    }

    /// Move into `folder`, unless somebody there has one of the same addresses: then ask first.
    /// True once moved.
    fn choose(&mut self, app: &mut MailApp, folder: Id) -> bool {
        if let Some((_, there)) = already_in(&app.session.mailbox, &self.ids, folder) {
            self.duplicate = Some((there, folder));
            return false;
        }
        self.go(app, folder)
    }

    /// "<name> is already in <folder>": Open that one, or Move anyway.
    fn ask(&mut self, app: &mut MailApp, ui: &mut Ui, there: Id, folder: Id) -> bool {
        let mailbox = &app.session.mailbox;
        let name = mailbox.contact(there).map(|c| c.name()).unwrap_or_default();
        let place = targets(mailbox).into_iter().find(|(id, _)| *id == folder).map(|(_, label)| label).unwrap_or_default();
        ui.label(crate::i18n::fmt(tl!("{name} is already in {folder}."), &[("name", &name), ("folder", &place)]));
        let (mut open, mut anyway) = (false, false);
        ui.add_space(4.0);
        crate::dialogs::button_row(ui, |ui| {
            open = ui.add(egui::Button::new(tl!("Open that one")).min_size(egui::vec2(72.0, 26.0))).clicked();
            anyway = crate::widgets::primary_button(ui, tl!("Move anyway"), true).clicked();
        });
        if open {
            let _ = app.execute("people.select", json!({"id": there}));
            return true;
        }
        anyway && self.go(app, folder)
    }

    fn go(&mut self, app: &mut MailApp, folder: Id) -> bool {
        match app.execute("people.moveToFolder", json!({"ids": self.ids, "folder": folder})) {
            Ok(said) => {
                app.people_picked.clear();
                // (The engine gives each name cleaned and short.)
                let skipped: Vec<&str> = said["skipped"].as_array().into_iter().flatten().filter_map(|c| c["name"].as_str()).collect();
                if !skipped.is_empty() {
                    let line = tln!(
                        skipped.len(),
                        "{n} contact wasn't moved, as it is still being sent to its server: {names}. Try again shortly.",
                        "{n} contacts weren't moved, as they are still being sent to their servers: {names}. Try again shortly."
                    );
                    let mut names = skipped.iter().take(NAMED).copied().collect::<Vec<_>>().join(", ");
                    let more = skipped.len().saturating_sub(NAMED);
                    if more > 0 {
                        names = crate::i18n::fmt(&tln!(more, "{people} and {n} more", "{people} and {n} more"), &[("people", &names)]);
                    }
                    app.status(crate::i18n::fmt(&line, &[("names", &names)]));
                }
                true
            }
            Err(e) => {
                (self.error, self.duplicate) = (e, None);
                false
            }
        }
    }
}

#[cfg(test)]
#[path = "move_contacts_tests.rs"]
mod move_contacts_tests;
