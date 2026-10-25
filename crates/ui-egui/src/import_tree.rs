// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Open & Export › Import a Folder of Mailboxes… (#420): the owner picks a folder, this
//! shows what is in it (the answer of `file.importFolderTree` with `dryRun`, so nothing has
//! changed yet), and Import runs the command. Names come from disk: they are shown as text, cut
//! to the column.

use egui::{RichText, Ui, vec2};
use serde_json::{Value, json};

use crate::MailApp;
use crate::dialogs::buttons;
use crate::theme::Tokens;

pub const WIDTH: f32 = 500.0;
/// Most folders listed; the rest are counted.
const LISTED: usize = 200;

pub struct ImportTree {
    path: String,
    /// What a dry run answered, or why it couldn't.
    preview: Result<Value, String>,
}

impl ImportTree {
    /// Look into `path` (the folder the owner picked) without changing anything.
    pub fn open(app: &mut MailApp, path: &str) -> ImportTree {
        let preview = app.session.run("file.importFolderTree", &json!({"path": path, "dryRun": true})).map_err(|e| e.to_string());
        ImportTree { path: path.to_string(), preview }
    }

    fn count(&self, key: &str) -> u64 {
        self.preview.as_ref().ok().and_then(|p| p.get(key)).and_then(Value::as_u64).unwrap_or(0)
    }

    /// The dialog's contents; `true` when it is done (imported, or cancelled).
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> bool {
        let t = Tokens::get(ui.ctx());
        let mut done = false;
        match &self.preview {
            Err(why) => {
                ui.label(RichText::new(why.as_str()).color(t.red));
            }
            Ok(p) => {
                let name = p.get("name").and_then(Value::as_str).unwrap_or("");
                ui.label(crate::i18n::fmt(tl!("Found in {path}. Nothing is imported until you press Import."), &[("path", &self.path)]));
                let folders = p.get("folders").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
                let total = self.count("messages");
                let dups = self.count("duplicates");
                ui.label(
                    RichText::new(crate::i18n::fmt(
                        tl!("{folders} will be made under a new folder called {name} on this computer, with {messages}."),
                        &[
                            ("folders", &tln!(folders.len() as u64, "{n} folder", "{n} folders")),
                            ("name", name),
                            ("messages", &tln!(total, "{n} message", "{n} messages")),
                        ],
                    ))
                    .strong(),
                );
                egui::ScrollArea::vertical().max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                    for f in folders.iter().take(LISTED) {
                        let path = f.get("path").and_then(Value::as_str).unwrap_or("");
                        let n = f.get("added").and_then(Value::as_u64).unwrap_or(0);
                        ui.horizontal(|ui| {
                            ui.allocate_ui_with_layout(vec2(WIDTH - 130.0, 18.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                ui.add(egui::Label::new(path).truncate());
                            });
                            ui.label(RichText::new(tln!(n, "{n} message", "{n} messages")).color(t.text_dim));
                        });
                    }
                    if folders.len() > LISTED {
                        ui.label(
                            RichText::new(tln!((folders.len() - LISTED) as u64, "and {n} more folder", "and {n} more folders")).color(t.text_dim),
                        );
                    }
                });
                if dups > 0 {
                    ui.label(
                        RichText::new(tln!(dups, "{n} message is already here and is left out", "{n} messages are already here and are left out"))
                            .color(t.text_dim),
                    );
                }
                let skipped = self.count("skippedCount");
                if skipped > 0 {
                    ui.label(
                        RichText::new(tln!(
                            skipped,
                            "{n} file or folder is left out: not a mailbox file, a link, or over a limit",
                            "{n} files or folders are left out: not mailbox files, links, or over a limit"
                        ))
                        .color(t.text_dim),
                    );
                }
                for note in p.get("notes").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).take(5) {
                    ui.label(RichText::new(note).color(t.text_dim));
                }
            }
        }
        let (ok, cancel) = buttons(ui, tl!("Import"), self.count("messages") > 0);
        if cancel {
            done = true;
        } else if (ok || (enter && self.count("messages") > 0)) && !done {
            // (Its own answer, a count and a note, is said in the status bar.)
            let _ = app.run("file.importFolderTree", json!({"path": self.path}));
            app.ui.backstage = false;
            done = true;
        }
        done
    }
}
