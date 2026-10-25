// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Info › Remove MailCraft's data from this computer (#177): everything MailCraft keeps
//! here, gone, for an installed or a portable copy alike.
//!
//! - **The command does the work, the dialog only asks.** `ui.removeAllData` removes the data;
//!   by hand (`run`) it first asks ([`crate::dialogs::confirmation`]). A script or the control
//!   channel (`execute`) runs it at once, without asking: whoever holds the window's key said
//!   what they meant. An agent may never run it (`NEVER` in the engine's `agent.rs`).
//! - **What goes where, in this order.** The window first makes sure the saver is idle (a save
//!   still running after its wait refuses the removal: nothing was touched, try again), then
//!   stops writing (no mailbox file any more, autosave off, no Send/Receive, nothing remembered
//!   again), asks the host ([`DataWipe`]) to remove its folders (the host alone knows them, and
//!   removes nothing outside the ones it resolves itself), and only then forgets the sign-ins
//!   the system's credential store keeps for every account here (through
//!   [`Services::secrets`]).
//! - **A failure is a line in a report, never a panic**, and the rest still goes. MailCraft quits
//!   once the report is read, or at once when there is nothing to report.
//! - Removing one account is `account.remove` (the same files, for that account alone).

use egui::Ui;
use serde_json::{Value, json};

use crate::MailApp;
use crate::dialogs::{Dialog, Question};
use crate::theme::Tokens;

/// What the host removed ([`DataWipe::erase`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WipeReport {
    /// Files and folders removed (each counted once, folders with what was in them).
    pub removed: usize,
    /// What couldn't be removed (its path) and why.
    pub failed: Vec<(String, Why)>,
    /// What was left alone on purpose (its path) and why.
    pub left: Vec<(String, Why)>,
}

/// Why something was not removed. The window says it in the interface language
/// ([`Why::text`]); only the operating system's own words ([`Why::Io`]) are as it gave them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// The system refused, in its words.
    Io(String),
    /// A link: it is removed itself and never followed.
    IsLink,
    /// Not a folder MailCraft keeps its data in: nothing in it is touched.
    NotOurs,
    NotFolder,
    TooDeep,
    TooMany,
    /// A path that was not inside the folder being cleared.
    Outside,
    /// A mailbox the owner keeps outside MailCraft's own folder (the file, its content folder
    /// and its word index): theirs.
    KeptByOwner,
    /// Not removed because the mailbox file of its folder could not be: a mailbox without its
    /// content would look whole and not be.
    MailboxStays,
}

impl Why {
    pub fn text(&self) -> String {
        match self {
            Why::Io(e) => e.clone(),
            Why::IsLink => tl!("is a link: not followed, and nothing in it was removed").to_string(),
            Why::NotOurs => tl!("isn't a folder MailCraft keeps its data in: left alone").to_string(),
            Why::NotFolder => tl!("isn't a folder: left alone").to_string(),
            Why::TooDeep => tl!("is nested too deep: left alone").to_string(),
            Why::TooMany => tl!("has too many files: the rest was not removed").to_string(),
            Why::Outside => tl!("is outside the folder being cleared: left alone").to_string(),
            Why::KeptByOwner => {
                tl!("is part of a mailbox you opened from outside MailCraft's own folder: it is yours and was not touched").to_string()
            }
            Why::MailboxStays => tl!("was kept because the mailbox file beside it could not be removed: remove them together").to_string(),
        }
    }
}

fn line((path, why): &(String, Why)) -> String {
    format!("{path}: {}", why.text())
}

/// The host's way of removing what MailCraft keeps on this computer ([`Services::data_wipe`]).
pub trait DataWipe {
    /// Remove the settings folder (the mailbox and its content, the preferences, the logs, the
    /// downloaded pictures) and the folder opened attachments are written to, and nothing else.
    /// `mailbox` is the file the window had open, if any: the host removes it only when it is
    /// in its own settings folder, and otherwise says that it was left.
    fn erase(&self, mailbox: Option<&std::path::Path>) -> WipeReport;
}

/// How long the saver is waited for to be idle, after its flush: a sweep may still be queued.
const IDLE_WAIT: std::time::Duration = if cfg!(test) { std::time::Duration::from_millis(100) } else { std::time::Duration::from_secs(5) };

impl MailApp {
    /// `ui.removeAllData`: remove all of MailCraft's data from this computer, then quit.
    pub(crate) fn remove_all_data(&mut self) -> Result<Value, String> {
        if self.data_removed {
            return Err(tl!("MailCraft's data has been removed already").to_string());
        }
        if self.services.data_wipe.is_none() {
            return Err(tl!("Removing MailCraft's data isn't available here").to_string());
        }
        // A save still running would write the mailbox back after (or while) it is removed: the
        // saver is waited for, and when it is not idle by then nothing has been touched.
        if let Some(saver) = &self.services.saver {
            saver.flush();
            let until = std::time::Instant::now() + IDLE_WAIT;
            while saver.progress().is_some() {
                if std::time::Instant::now() >= until {
                    return Err(tl!("A save of the mailbox is still running: try again in a moment").to_string());
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        // Nothing writes any more: no Send/Receive, no mailbox file, nothing remembered.
        self.data_removed = true;
        self.services.sync_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        self.sync_queue.clear();
        let mailbox = self.session.path.take();
        self.autosave = false;
        self.session.compose = None;
        let Some(host) = self.services.data_wipe.as_ref() else { return Err(String::new()) };
        let mut report = host.erase(mailbox.as_deref());
        // Only now the sign-ins the system remembers: every entry MailCraft noted putting there,
        // whichever mailbox it was for, and those of the accounts here. What stays is listed.
        for key in self.forget_every_secret() {
            report.failed.push((
                crate::i18n::fmt(tl!("The system's credential store: {key}"), &[("key", &key)]),
                Why::Io(tl!("it could not be removed").to_string()),
            ));
        }
        self.passwords.clear();
        self.tokens.clear();
        self.graph_tokens.clear();
        if let Some(trouble) = self.services.secrets.as_ref().and_then(|store| store.take_trouble()) {
            report.failed.push((tl!("The system's credential store").to_string(), Why::Io(trouble.notice())));
        }
        if !report.failed.is_empty() || !report.left.is_empty() {
            self.dialog = Some(Dialog::Report { name: "dataRemoval", title: tl!("Remove MailCraft's Data").to_string(), lines: lines(&report) });
        }
        let said = |list: &[(String, Why)]| list.iter().map(line).collect::<Vec<_>>();
        Ok(json!({"removed": report.removed, "failed": said(&report.failed), "left": said(&report.left)}))
    }

    /// Once the data is gone and nothing is left to read, MailCraft goes.
    pub(crate) fn data_removal_tick(&mut self) {
        if self.data_removed && self.dialog.is_none() {
            self.quit_requested = true;
        }
    }
}

/// The report, as the lines the owner reads.
fn lines(report: &WipeReport) -> Vec<String> {
    let mut lines = Vec::new();
    if !report.failed.is_empty() {
        lines.push(tl!("These could not be removed. Remove them yourself:").to_string());
        lines.extend(report.failed.iter().map(line));
    }
    if !report.left.is_empty() {
        lines.push(tl!("These were left alone:").to_string());
        lines.extend(report.left.iter().map(line));
    }
    // By outcome: a folder that was left (a link, a locked file) is still there.
    if report.failed.is_empty() {
        lines.push(tl!("Everything else is gone. MailCraft quits when you close this.").to_string());
    } else {
        lines.push(tl!("Some of it is still there, as listed above. MailCraft quits when you close this.").to_string());
    }
    lines
}

/// What the owner is asked before the data goes.
pub(crate) fn question() -> Question {
    Question::new(
        tl!("Remove MailCraft's Data"),
        format!(
            "{}\n\n{}\n\n{}",
            tl!(
                "This removes everything MailCraft keeps on this computer: every account's mail, attachments, contacts, calendar and settings, the sign-ins remembered in the system's credential store, downloaded pictures, opened attachments and logs."
            ),
            tl!("It is the same for an installed and a portable copy. MailCraft quits afterwards."),
            tl!("Mail on the servers is not touched. This can't be undone."),
        ),
        tl!("Remove Everything"),
        Value::Null,
    )
}

/// File › Info: the section with the button.
pub(crate) fn section(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    if app.services.data_wipe.is_none() {
        return;
    }
    ui.add_space(10.0);
    ui.label(egui::RichText::new(tl!("Data on this computer")).font(crate::theme::semibold(14.0)).color(t.accent_text));
    ui.label(
        egui::RichText::new(tl!(
            "Take everything MailCraft keeps here with you when you are done: for a shared or work computer, or before deleting a portable copy."
        ))
        .color(t.text_dim),
    );
    if ui.button(tl!("Remove MailCraft's data from this computer…")).on_hover_text(tl!("Mail on the servers is not touched.")).clicked() {
        let _ = app.run("ui.removeAllData", json!({}));
    }
}

#[cfg(test)]
#[path = "data_removal_tests.rs"]
mod tests;
