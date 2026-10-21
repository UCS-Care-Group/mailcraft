// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Info › Remove MailCraft's data from this computer (#177): everything MailCraft keeps
//! here, gone, for an installed or a portable copy alike.
//!
//! - **The command does the work, the dialog only asks.** `ui.removeAllData` removes the data;
//!   by hand (`run`) it first asks ([`crate::dialogs::confirmation`]), for a script it never
//!   does, and an agent never may run it (`NEVER` in the engine's `agent.rs`).
//! - **What goes where.** The window stops writing (the saver is let finish, then the mailbox has
//!   no file any more, autosave is off, nothing is remembered again), forgets the sign-ins the
//!   system's credential store keeps for every account here (through [`Services::secrets`]), and
//!   then asks the host ([`DataWipe`]) to remove its folders: the host alone knows them, and
//!   removes nothing outside the ones it resolves itself.
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
    /// A mailbox file the owner keeps outside MailCraft's own folder: theirs.
    KeptByOwner,
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
            Why::KeptByOwner => tl!("is a mailbox file you opened from outside MailCraft's own folder: it is yours and was not touched").to_string(),
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

impl MailApp {
    /// `ui.removeAllData`: remove all of MailCraft's data from this computer, then quit.
    pub(crate) fn remove_all_data(&mut self) -> Result<Value, String> {
        if self.data_removed {
            return Err(tl!("MailCraft's data has been removed already").to_string());
        }
        let Some(host) = self.services.data_wipe.as_ref() else {
            return Err(tl!("Removing MailCraft's data isn't available here").to_string());
        };
        // Nothing writes any more: no Send/Receive brings anything down, the saver is let finish
        // what it is at and then has no file to write to, and nothing is remembered again.
        self.services.sync_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(saver) = &self.services.saver {
            saver.flush();
        }
        let mailbox = self.session.path.take();
        self.autosave = false;
        self.data_removed = true;
        self.session.compose = None;
        // The sign-ins the system remembers, for every account here (and so for each shared
        // mailbox's address, which only ever had its owner's).
        let addresses: Vec<String> = self.session.mailbox.accounts.iter().map(|a| a.email.clone()).filter(|e| !e.trim().is_empty()).collect();
        for address in &addresses {
            self.forget_secrets(address);
        }
        self.passwords.clear();
        self.tokens.clear();
        let mut report = host.erase(mailbox.as_deref());
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
    lines.push(tl!("Everything else is gone. MailCraft quits when you close this.").to_string());
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
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use mailcraft_engine::Session;

    use super::*;
    use crate::{SecretStore, Services};

    const NOW: i64 = 1_791_993_600;

    /// The system's store, in memory.
    #[derive(Clone, Default)]
    struct Store(Arc<Mutex<HashMap<String, String>>>);

    impl SecretStore for Store {
        fn get(&self, key: &str) -> Option<String> {
            self.0.lock().unwrap().get(key).cloned()
        }
        fn set(&self, key: &str, secret: &str) -> Result<(), String> {
            self.0.lock().unwrap().insert(key.to_string(), secret.to_string());
            Ok(())
        }
        fn delete(&self, key: &str) {
            self.0.lock().unwrap().remove(key);
        }
    }

    /// A host that removes nothing and says what it was asked, and answers what it is told.
    #[derive(Clone, Default)]
    struct Host {
        asked: Arc<Mutex<Vec<Option<PathBuf>>>>,
        answer: Arc<Mutex<WipeReport>>,
    }

    impl DataWipe for Host {
        fn erase(&self, mailbox: Option<&Path>) -> WipeReport {
            self.asked.lock().unwrap().push(mailbox.map(Path::to_path_buf));
            self.answer.lock().unwrap().clone()
        }
    }

    fn app(host: &Host, store: &Store) -> MailApp {
        let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
        a.autosave = false;
        a.services.data_wipe = Some(Box::new(host.clone()));
        a.services.secrets = Some(Box::new(store.clone()));
        a
    }

    #[test]
    fn by_hand_it_asks_first_and_a_script_is_never_asked() {
        let (host, store) = (Host::default(), Store::default());
        let mut a = app(&host, &store);
        assert_eq!(a.run("ui.removeAllData", json!({})).unwrap()["pending"], "confirm");
        let Some(Dialog::Confirm { ask, .. }) = &a.dialog else { panic!("no question") };
        for said in [
            "every account's mail",
            "remembered in the system's credential store",
            "downloaded pictures",
            "opened attachments",
            "logs",
            "Mail on the servers is not touched",
        ] {
            assert!(ask.text.contains(said), "{said}: {}", ask.text);
        }
        assert!(host.asked.lock().unwrap().is_empty(), "removed before the answer");
        assert!(!a.data_removed);
        // The answer is the command with "confirmed".
        a.dialog = None;
        a.run("ui.removeAllData", json!({"confirmed": true})).unwrap();
        assert!(a.data_removed);
        assert_eq!(host.asked.lock().unwrap().len(), 1);
    }

    #[test]
    fn an_agent_may_never_run_it_and_nothing_is_offered_without_a_host() {
        assert_eq!(mailcraft_engine::agent::access("ui.removeAllData"), Some(mailcraft_engine::agent::Access::Never));
        let (host, store) = (Host::default(), Store::default());
        let mut a = app(&host, &store);
        a.services.data_wipe = None;
        assert!(a.run("ui.removeAllData", json!({})).is_err());
        assert!(a.dialog.is_none());
        assert!(a.execute("ui.removeAllData", json!({})).is_err());
        assert!(!a.data_removed);
    }

    #[test]
    fn the_remembered_sign_ins_of_every_account_go_and_nothing_is_remembered_again() {
        let (host, store) = (Host::default(), Store::default());
        let mut a = app(&host, &store);
        a.execute("account.add", json!({"email": "Avery@Mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap();
        a.execute("account.add", json!({"email": "quinn@mail.example", "auth": "oauth2", "incoming": {"host": "imap.mail.example"}})).unwrap();
        store.set("password:avery@mail.example", "pw").unwrap();
        store.set("refresh:quinn@mail.example", "ref").unwrap();
        // Someone else's entry in the same store is none of ours.
        store.set("password:other", "x").ok();
        a.passwords.insert(7, "in memory".into());
        a.ui.remember_sign_in = true;

        a.execute("ui.removeAllData", json!({})).unwrap();

        let left: Vec<String> = store.0.lock().unwrap().keys().filter(|k| k.contains("mail.example")).cloned().collect();
        assert!(left.is_empty(), "{left:?}");
        assert!(a.passwords.is_empty() && a.tokens.is_empty());
        // A Send/Receive that was still under way can't put one back.
        a.remember("password", "avery@mail.example", "late");
        assert!(store.get("password:avery@mail.example").is_none());
        // And nothing is written on the way out.
        assert_eq!(a.save_on_exit(), Ok(()));
        assert!(a.session.path.is_none() && !a.autosave);
    }

    #[test]
    fn the_host_is_told_which_mailbox_was_open_and_the_window_quits_when_nothing_is_left_to_read() {
        let (host, store) = (Host::default(), Store::default());
        let mut a = app(&host, &store);
        a.session.path = Some(PathBuf::from("/somewhere/mailbox.mailcraft"));
        a.execute("ui.removeAllData", json!({})).unwrap();
        assert_eq!(host.asked.lock().unwrap().as_slice(), [Some(PathBuf::from("/somewhere/mailbox.mailcraft"))]);
        assert!(a.dialog.is_none());
        a.data_removal_tick();
        assert!(a.quit_requested);
        // Not again.
        assert!(a.execute("ui.removeAllData", json!({})).is_err());
        assert_eq!(host.asked.lock().unwrap().len(), 1);
    }

    #[test]
    fn what_could_not_be_removed_is_reported_and_the_window_waits_for_it_to_be_read() {
        let (host, store) = (Host::default(), Store::default());
        *host.answer.lock().unwrap() = WipeReport {
            removed: 3,
            failed: vec![("/home/ada/.config/mailcraft/logs/mailcraft.log".into(), Why::Io("permission denied".into()))],
            left: vec![("/usb/mail.mailcraft".into(), Why::KeptByOwner)],
        };
        let mut a = app(&host, &store);
        let said = a.execute("ui.removeAllData", json!({})).unwrap();
        assert_eq!(said["removed"], 3);
        assert_eq!(said["failed"].as_array().unwrap().len(), 1);
        let Some(Dialog::Report { name, lines, .. }) = &a.dialog else { panic!("no report") };
        assert_eq!(*name, "dataRemoval");
        let text = lines.join("\n");
        assert!(text.contains("mailcraft.log") && text.contains("permission denied") && text.contains("/usb/mail.mailcraft"), "{text}");
        a.data_removal_tick();
        assert!(!a.quit_requested, "quit before the report was read");
        a.dialog = None;
        a.data_removal_tick();
        assert!(a.quit_requested);
    }

    #[test]
    fn an_unreachable_credential_store_is_reported_not_hidden() {
        struct Locked;
        impl SecretStore for Locked {
            fn get(&self, _: &str) -> Option<String> {
                None
            }
            fn set(&self, _: &str, _: &str) -> Result<(), String> {
                Err("locked".into())
            }
            fn delete(&self, _: &str) {}
            fn take_trouble(&self) -> Option<crate::StoreTrouble> {
                Some(crate::StoreTrouble::Locked)
            }
        }
        let (host, store) = (Host::default(), Store::default());
        let mut a = app(&host, &store);
        a.services.secrets = Some(Box::new(Locked));
        let said = a.execute("ui.removeAllData", json!({})).unwrap();
        assert_eq!(said["failed"].as_array().unwrap().len(), 1, "{said}");
        assert!(matches!(a.dialog, Some(Dialog::Report { .. })));
    }

    #[test]
    fn removing_an_account_forgets_its_sign_in_and_cannot_be_undone_into_its_mail() {
        let (host, store) = (Host::default(), Store::default());
        let mut a = app(&host, &store);
        let id = a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"]
            .as_u64()
            .unwrap();
        store.set("password:avery@mail.example", "pw").unwrap();
        store.set("refresh:avery@mail.example", "ref").unwrap();
        a.execute("account.remove", json!({"id": id})).unwrap();
        assert!(store.0.lock().unwrap().is_empty());
        assert!(!a.session.can_undo() && !a.session.can_redo());
        assert!(a.session.mailbox.account(id).is_none());
    }
}
