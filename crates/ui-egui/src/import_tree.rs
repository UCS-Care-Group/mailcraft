// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Open & Export › Import a Folder of Mailboxes… (#420): the owner picks a folder, this
//! shows what is in it (the answer of `file.importFolderTree` with `dryRun`, so nothing has
//! changed yet), and Import runs the command. Names come from disk: they are shown as text, cut
//! to the column.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{RichText, Ui, vec2};
use mailcraft_engine::import_tree::{Parsed, Progress};
use serde_json::{Value, json};

use crate::MailApp;
use crate::dialogs::{button_row, buttons};
use crate::theme::Tokens;

pub const WIDTH: f32 = 500.0;
/// Most folders listed; the rest are counted.
const LISTED: usize = 200;

/// What the host is asked to do off the window's thread: walk `path`, read every mbox file in it
/// and parse the messages. `count_only` is the look before an import (nothing kept but counts and
/// Message-IDs). The window shows `progress` and cancels through it.
pub struct TreeJob {
    pub path: String,
    pub count_only: bool,
    pub progress: Arc<Progress>,
}

/// What the dialog is doing.
enum Phase {
    /// Looking into the folder (off the window's thread).
    Scanning(Receiver<Result<Parsed, String>>),
    /// What a dry run answered, or why it couldn't: waiting for Import.
    Preview(Result<Value, String>),
    /// Reading the files in full (off the window's thread); nothing is in the mailbox until it is done.
    Importing(Receiver<Result<Parsed, String>>),
}

pub struct ImportTree {
    path: String,
    phase: Phase,
    progress: Arc<Progress>,
    /// The mailbox this was opened over: another one opened since drops the result.
    opening: u64,
    /// Messages the look found, the whole of the import's progress bar.
    total: u64,
    /// Set when the mailbox was swapped or the work was cancelled: the dialog closes.
    dropped: bool,
}

impl Drop for ImportTree {
    /// A dialog that goes away (Cancel, Escape, the close button) stops what it started.
    fn drop(&mut self) {
        self.progress.cancel();
    }
}

impl ImportTree {
    /// Look into `path` (the folder the owner picked) without changing anything. The look runs
    /// on the host's thread when it has one; otherwise it is done here, at once.
    pub fn open(app: &mut MailApp, path: &str) -> ImportTree {
        let mut d = ImportTree {
            path: path.to_string(),
            phase: Phase::Preview(Err(String::new())),
            progress: Arc::default(),
            opening: app.session.openings(),
            total: 0,
            dropped: false,
        };
        d.start(app, true);
        d
    }

    /// Start the work off the window's thread, or, where there is no host for it, do it now.
    fn start(&mut self, app: &mut MailApp, count_only: bool) {
        self.progress = Arc::default();
        if let Some(spawn) = &app.services.import_tree {
            let rx = spawn(TreeJob { path: self.path.clone(), count_only, progress: self.progress.clone() });
            self.phase = if count_only { Phase::Scanning(rx) } else { Phase::Importing(rx) };
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        let done = mailcraft_engine::import_tree::load_path(&self.path, None, count_only, &self.progress).map_err(|e| e.to_string());
        #[cfg(target_arch = "wasm32")]
        let done: Result<Parsed, String> = Err(tl!("the browser can't read folders by path").to_string());
        self.arrived(app, count_only, done);
    }

    /// The work is done: show the look, or put the tree into the mailbox in one step.
    fn arrived(&mut self, app: &mut MailApp, count_only: bool, done: Result<Parsed, String>) {
        if self.progress.is_cancelled() {
            self.dropped = true;
            return;
        }
        match done {
            Err(why) => self.phase = Phase::Preview(Err(why)),
            Ok(parsed) if count_only => {
                let preview = mailcraft_engine::import_tree::finish(&mut app.session, parsed, true).map_err(|e| e.to_string());
                self.total = preview.as_ref().ok().and_then(|p| p.get("messages")).and_then(Value::as_u64).unwrap_or(0);
                self.phase = Phase::Preview(preview);
            }
            Ok(parsed) => {
                // The one change: the engine puts the staged tree in as a single undo step.
                // (Its own answer, a count and a note, is said in the status bar.)
                app.session.staged_tree = Some(parsed);
                let _ = app.run("file.importFolderTree", json!({"staged": true}));
                app.session.staged_tree = None;
                app.ui.backstage = false;
                self.dropped = true;
            }
        }
    }

    /// Take in what the thread has sent, if anything.
    fn poll(&mut self, app: &mut MailApp) {
        if self.opening != app.session.openings() {
            // Another mailbox was opened meanwhile: what comes back is not for this one.
            self.progress.cancel();
            self.dropped = true;
            return;
        }
        let (rx, count_only) = match &self.phase {
            Phase::Scanning(rx) => (rx, true),
            Phase::Importing(rx) => (rx, false),
            Phase::Preview(_) => return,
        };
        let done = match rx.try_recv() {
            Ok(done) => done,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(tl!("it stopped unexpectedly").to_string()),
        };
        self.phase = Phase::Preview(Err(String::new()));
        self.arrived(app, count_only, done);
    }

    /// The line that says how far the work is.
    fn progress_text(&self, importing: bool) -> String {
        let (folders, files, messages) = self.progress.counts();
        if importing {
            crate::i18n::fmt(
                tl!("Importing… {messages} of {total}"),
                &[("messages", &tln!(messages as u64, "{n} message", "{n} messages")), ("total", &self.total.to_string())],
            )
        } else {
            crate::i18n::fmt(
                tl!("Looking… {folders}, {files}, {messages} so far"),
                &[
                    ("folders", &tln!(folders as u64, "{n} folder", "{n} folders")),
                    ("files", &tln!(files as u64, "{n} mailbox file", "{n} mailbox files")),
                    ("messages", &tln!(messages as u64, "{n} message", "{n} messages")),
                ],
            )
        }
    }

    fn count(&self, key: &str) -> u64 {
        match &self.phase {
            Phase::Preview(Ok(p)) => p.get(key).and_then(Value::as_u64).unwrap_or(0),
            _ => 0,
        }
    }

    /// The dialog's contents; `true` when it is done (imported, or cancelled).
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> bool {
        // Room round the text, so that it doesn't sit against the window's edge.
        egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| self.contents(app, ui, enter)).inner
    }

    fn contents(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> bool {
        let t = Tokens::get(ui.ctx());
        let mut done = false;
        self.poll(app);
        if self.dropped {
            return true;
        }
        let importing = match &self.phase {
            Phase::Preview(_) => None,
            Phase::Scanning(_) => Some(false),
            Phase::Importing(_) => Some(true),
        };
        if let Some(importing) = importing {
            // Working off the window's thread: say how far it is, and let the owner stop it.
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.label(self.progress_text(importing));
            });
            if importing {
                let (_, _, messages) = self.progress.counts();
                ui.add(egui::ProgressBar::new((messages as f32 / self.total.max(1) as f32).clamp(0.0, 1.0)));
                ui.label(RichText::new(tl!("Nothing is added to your mail until it has all been read.")).color(t.text_dim));
            }
            let mut cancel = false;
            ui.add_space(4.0);
            button_row(ui, |ui| cancel = ui.add(egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0))).clicked());
            return cancel;
        }
        let Phase::Preview(preview) = &self.phase else { return false };
        match preview {
            Err(why) if why.is_empty() => {}
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
            // Read in full off the window's thread; `arrived` puts it in when it is all here.
            self.start(app, false);
        }
        done
    }
}

#[cfg(test)]
#[path = "import_tree_tests.rs"]
mod import_tree_tests;
