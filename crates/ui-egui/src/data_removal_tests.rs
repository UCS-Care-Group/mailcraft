// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Remove MailCraft's data from this computer (#177): what the window does, with a fake host and
//! a fake credential store.

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
    store.set("password:avery@mail.example|imap=imap.mail.example:0|smtp=none", "pw").unwrap();
    store.set("refresh:quinn@mail.example|imap=imap.mail.example:0|smtp=none", "ref").unwrap();
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
    assert!(store.get("password:avery@mail.example|imap=imap.mail.example:0|smtp=none").is_none());
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

/// A store that is locked: it gives nothing back, removes nothing, and says why.
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

#[test]
fn an_unreachable_credential_store_is_reported_not_hidden() {
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
    let id =
        a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    store.set("password:avery@mail.example|imap=imap.mail.example:0|smtp=none", "pw").unwrap();
    store.set("refresh:avery@mail.example|imap=imap.mail.example:0|smtp=none", "ref").unwrap();
    a.execute("account.remove", json!({"id": id})).unwrap();
    assert!(store.0.lock().unwrap().is_empty());
    assert!(!a.session.can_undo() && !a.session.can_redo());
    assert!(a.session.mailbox.account(id).is_none());
}

#[test]
fn removing_the_only_account_forgets_its_sign_in_and_leaves_an_empty_mailbox() {
    let (host, store) = (Host::default(), Store::default());
    let mut a = app(&host, &store);
    a.session = Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0);
    let id =
        a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    store.set("password:avery@mail.example", "pw").unwrap();
    store.set("refresh:avery@mail.example", "ref").unwrap();
    a.execute("account.remove", json!({"id": id})).unwrap();
    assert!(store.0.lock().unwrap().is_empty());
    assert!(matches!(a.session.mailbox.accounts.as_slice(), [fresh] if fresh.email.is_empty() && fresh.id != id));
    assert!(!a.session.can_undo() && !a.session.can_redo());
    // Nothing of a run of it is left waiting either: there is no account here to sync.
    assert!(a.server_accounts().is_empty());
}

// ---- the review of #294 ----

/// A saver that is never idle: a save that is still running when the wait is over.
struct Busy;

impl mailcraft_engine::io::BackgroundSave for Busy {
    fn follow(&self, _: &Path) {}
    fn submit(&self, _: &Path, _: &mailcraft_model::Mailbox, _: u64) {}
    fn reports(&self) -> Vec<mailcraft_engine::io::SaveReport> {
        Vec::new()
    }
    fn progress(&self) -> Option<mailcraft_engine::io::SaveProgress> {
        Some(Default::default())
    }
    fn flush(&self) {}
}

#[test]
fn a_save_still_running_refuses_the_removal_and_touches_nothing() {
    let (host, store) = (Host::default(), Store::default());
    let mut a = app(&host, &store);
    a.services.saver = Some(Box::new(Busy));
    a.session.path = Some(PathBuf::from("/somewhere/mailbox.mailcraft"));
    store.set("password:avery@lumen.example", "pw").unwrap();
    let refused = a.execute("ui.removeAllData", json!({}));
    assert!(refused.is_err(), "{refused:?}");
    assert!(host.asked.lock().unwrap().is_empty(), "the host removed with a save in flight");
    assert!(!a.data_removed && a.session.path.is_some());
    assert!(store.get("password:avery@lumen.example").is_some(), "a sign-in went with nothing removed");
    // It can be tried again: the same command, once the saver is idle.
    a.services.saver = None;
    assert!(a.execute("ui.removeAllData", json!({})).is_ok());
}

#[test]
fn sign_ins_go_after_the_files_and_not_before() {
    let log: Arc<Mutex<Vec<&'static str>>> = Arc::default();
    struct Ordered(Arc<Mutex<Vec<&'static str>>>);
    impl DataWipe for Ordered {
        fn erase(&self, _: Option<&Path>) -> WipeReport {
            self.0.lock().unwrap().push("files");
            WipeReport::default()
        }
    }
    struct Watching(Arc<Mutex<Vec<&'static str>>>);
    impl SecretStore for Watching {
        fn get(&self, _: &str) -> Option<String> {
            None
        }
        fn set(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn delete(&self, _: &str) {
            let mut log = self.0.lock().unwrap();
            if log.last() != Some(&"sign-ins") {
                log.push("sign-ins");
            }
        }
    }
    let (host, store) = (Host::default(), Store::default());
    let mut a = app(&host, &store);
    a.services.data_wipe = Some(Box::new(Ordered(log.clone())));
    a.services.secrets = Some(Box::new(Watching(log.clone())));
    a.execute("ui.removeAllData", json!({})).unwrap();
    assert_eq!(log.lock().unwrap().as_slice(), ["files", "sign-ins"]);
}

#[test]
fn the_last_line_says_what_became_of_it() {
    let gone = lines(&WipeReport { removed: 2, left: vec![("/usb/m".into(), Why::KeptByOwner)], ..Default::default() });
    assert!(gone.last().unwrap().contains("Everything else is gone"), "{gone:?}");
    let link = lines(&WipeReport { failed: vec![("/home/ada/.config/mailcraft".into(), Why::IsLink)], ..Default::default() });
    let last = link.last().unwrap();
    assert!(last.contains("still there") && !last.contains("is gone"), "{last}");
}

#[test]
fn nothing_starts_sending_or_receiving_once_the_data_is_removed() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (host, store) = (Host::default(), Store::default());
    let mut a = app(&host, &store);
    a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap();
    let started = Arc::new(AtomicUsize::new(0));
    let seen = started.clone();
    a.services.sync = Some(Box::new(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
        std::sync::mpsc::channel().1
    }));
    a.auto_sync = true;
    a.sync_queue.push_back((1, true));
    a.execute("ui.removeAllData", json!({})).unwrap();
    assert!(a.sync_queue.is_empty(), "a run was still waiting its turn");
    // Every way a run starts: the button and the command, and the timer in every frame.
    assert!(!a.enabled("ui.sync"));
    assert!(a.execute("ui.sync", json!({"password": "x"})).is_err());
    assert!(a.run("ui.sync", json!({})).is_err());
    let ctx = egui::Context::default();
    for _ in 0..3 {
        a.logic(&ctx);
    }
    assert_eq!(started.load(Ordering::SeqCst), 0, "a Send/Receive started after the removal");
}

#[test]
fn what_was_remembered_for_a_mailbox_that_is_no_longer_open_goes_too() {
    let (host, store) = (Host::default(), Store::default());
    let mut a = app(&host, &store);
    a.ui.remember_sign_in = true;
    a.execute("account.add", json!({"email": "avery@mail.example", "auth": "oauth2", "incoming": {"host": "imap.mail.example"}})).unwrap();
    a.remember("refresh", "avery@mail.example", "invented-refresh-token");
    assert_eq!(store.0.lock().unwrap().len(), 1);
    // Another mailbox takes its place; the entry is still in the store.
    a.session.replace_mailbox(mailcraft_model::Mailbox::with_local_account("", ""));
    // The settings file names the entry and holds nothing secret.
    let saved = serde_json::to_string(&a.ui).unwrap();
    assert!(saved.contains("avery@mail.example") && !saved.contains("invented-refresh-token"));

    a.execute("ui.removeAllData", json!({})).unwrap();
    assert!(store.0.lock().unwrap().is_empty(), "{:?}", store.0.lock().unwrap().keys());
}

#[test]
fn turning_remembering_off_forgets_the_entries_of_other_mailboxes_too() {
    let (host, store) = (Host::default(), Store::default());
    let mut a = app(&host, &store);
    a.ui.remember_sign_in = true;
    a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap();
    a.remember("password", "avery@mail.example", "invented");
    a.session.replace_mailbox(mailcraft_model::Mailbox::with_local_account("", ""));
    a.execute("ui.rememberSignIn", json!({"value": false})).unwrap();
    assert!(store.0.lock().unwrap().is_empty());
    assert!(a.ui.remembered_keys.is_empty());
}

/// A store that cannot delete.
struct Stuck(Store);

impl SecretStore for Stuck {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key)
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        self.0.set(key, secret)
    }
    fn delete(&self, _: &str) {}
}

#[test]
fn an_entry_that_cannot_be_removed_is_in_the_report() {
    let (host, store) = (Host::default(), Store::default());
    let mut a = app(&host, &store);
    a.services.secrets = Some(Box::new(Stuck(store.clone())));
    a.ui.remember_sign_in = true;
    a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap();
    a.remember("password", "avery@mail.example", "invented");
    let said = a.execute("ui.removeAllData", json!({})).unwrap();
    assert!(said["failed"].as_array().is_some_and(|f| f.iter().any(|l| l.as_str().is_some_and(|l| l.contains("avery@mail.example")))), "{said}");
}

/// #325: removing an account also takes the names of its entries in the credential store off the
/// settings, so nothing there names the account any more; another account's stay.
#[test]
fn removing_an_account_takes_its_entry_names_off_the_settings() {
    let (host, store) = (Host::default(), Store::default());
    let mut a = app(&host, &store);
    a.ui.remember_sign_in = true;
    let id =
        a.execute("account.add", json!({"email": "avery@mail.example", "auth": "oauth2", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"]
            .as_u64()
            .unwrap();
    a.execute("account.add", json!({"email": "kit@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap();
    a.remember("refresh", "avery@mail.example", "invented-refresh-token");
    a.remember("password", "kit@mail.example", "invented");
    assert_eq!(a.ui.remembered_keys.len(), 2);

    a.execute("account.remove", json!({"id": id})).unwrap();
    let saved = serde_json::to_string(&a.ui).unwrap();
    assert!(!saved.contains("avery@mail.example"), "{saved}");
    assert!(matches!(a.ui.remembered_keys.as_slice(), [kit] if kit.contains("kit@mail.example")), "{:?}", a.ui.remembered_keys);
    assert_eq!(store.0.lock().unwrap().len(), 1);
}

/// A name whose entry the store couldn't remove stays, for Remove MailCraft's data to try again.
#[test]
fn a_name_whose_entry_stays_stays_too() {
    let (host, store) = (Host::default(), Store::default());
    let mut a = app(&host, &store);
    a.ui.remember_sign_in = true;
    let id =
        a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    a.remember("password", "avery@mail.example", "invented");
    let names = a.ui.remembered_keys.clone();
    assert_eq!(names.len(), 1);
    a.services.secrets = Some(Box::new(Stuck(store.clone())));

    a.execute("account.remove", json!({"id": id})).unwrap();
    assert_eq!(a.ui.remembered_keys, names);
    assert_eq!(store.0.lock().unwrap().len(), 1);
}

/// A store that couldn't be used (locked, or not answering) says nothing of what it keeps: every
/// name stays, and the owner hears why.
#[test]
fn a_store_that_cannot_be_used_keeps_every_name() {
    let (host, store) = (Host::default(), Store::default());
    let mut a = app(&host, &store);
    a.ui.remember_sign_in = true;
    let id =
        a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    a.remember("password", "avery@mail.example", "invented");
    let names = a.ui.remembered_keys.clone();
    assert_eq!(names.len(), 1);
    a.services.secrets = Some(Box::new(Locked));

    a.execute("account.remove", json!({"id": id})).unwrap();
    assert_eq!(a.ui.remembered_keys, names);
    assert_eq!(a.secrets_trouble, Some(crate::StoreTrouble::Locked));
    assert_eq!(a.status_msg.as_ref().map(|(text, _)| text.clone()), Some(crate::StoreTrouble::Locked.notice()));
}

/// Open MailCraft's data folder (#460): a host that opens nothing and notes which folder it was
/// asked to show, and fails when told to.
fn folder_host(path: impl Into<PathBuf>, fails: bool) -> (DataFolder, Arc<Mutex<Vec<PathBuf>>>) {
    let asked: Arc<Mutex<Vec<PathBuf>>> = Default::default();
    let noted = asked.clone();
    let open = Box::new(move |p: &Path| {
        noted.lock().unwrap().push(p.to_path_buf());
        if fails { Err(FolderTrouble::NoFileManager) } else { Ok(()) }
    });
    (DataFolder { path: path.into(), open }, asked)
}

/// A whole path on this platform: `at("home/ada")` is `/home/ada`, or `C:\home\ada` on Windows.
fn at(parts: &str) -> PathBuf {
    let root = if cfg!(windows) { PathBuf::from("C:\\") } else { PathBuf::from("/") };
    parts.split('/').fold(root, |path, part| path.join(part))
}

#[test]
fn the_data_folder_the_host_resolved_is_the_one_it_is_asked_to_show() {
    let (host, store) = (Host::default(), Store::default());
    let mut a = app(&host, &store);
    let (folder, asked) = folder_host(at("home/ada/.config/mailcraft"), false);
    a.services.data_folder = Some(folder);
    assert!(a.enabled("ui.openDataFolder"));
    // No mailbox file (the sample): the settings folder.
    assert!(a.session.path.is_none());
    // By hand nothing is asked first: it only shows a folder.
    assert_eq!(a.run("ui.openDataFolder", json!({})).unwrap()["opened"], "settings");
    assert!(a.dialog.is_none());
    // A param naming another folder is not looked at.
    a.execute("ui.openDataFolder", json!({"path": "/etc"})).unwrap();
    assert_eq!(asked.lock().unwrap().as_slice(), [at("home/ada/.config/mailcraft"), at("home/ada/.config/mailcraft")]);
    // Nothing of the data was touched.
    assert!(host.asked.lock().unwrap().is_empty() && !a.data_removed);
}

#[test]
fn the_data_folder_is_refused_without_a_host_for_an_agent_and_once_the_data_is_gone() {
    assert_eq!(mailcraft_engine::agent::access("ui.openDataFolder"), Some(mailcraft_engine::agent::Access::Never));
    let (host, store) = (Host::default(), Store::default());
    // The browser and a run that keeps nothing: no folder, so nothing is offered.
    let mut a = app(&host, &store);
    assert!(!a.enabled("ui.openDataFolder"));
    assert!(a.execute("ui.openDataFolder", json!({})).is_err());
    // A folder that isn't a whole path is never handed to the opener.
    let (folder, asked) = folder_host("relative/mailcraft", false);
    a.services.data_folder = Some(folder);
    assert!(a.execute("ui.openDataFolder", json!({})).is_err());
    assert!(asked.lock().unwrap().is_empty());
    // What the host couldn't do is said.
    let (folder, _) = folder_host(at("home/ada/.config/mailcraft"), true);
    a.services.data_folder = Some(folder);
    assert_eq!(a.execute("ui.openDataFolder", json!({})).unwrap_err(), FolderTrouble::NoFileManager.text());
    // Once the data is removed there is no folder to show.
    let (folder, asked) = folder_host(at("home/ada/.config/mailcraft"), false);
    a.services.data_folder = Some(folder);
    a.execute("ui.removeAllData", json!({})).unwrap();
    assert!(!a.enabled("ui.openDataFolder"));
    assert!(a.execute("ui.openDataFolder", json!({})).is_err());
    assert!(asked.lock().unwrap().is_empty());
}

#[test]
fn a_mailbox_kept_outside_the_settings_folder_has_its_own_folder_shown() {
    let (host, store) = (Host::default(), Store::default());
    let mut a = app(&host, &store);
    let (folder, asked) = folder_host(at("home/ada/.config/mailcraft"), false);
    a.services.data_folder = Some(folder);
    // `mailcraft --mailbox ~/Documents/work.mailcraft`: that mailbox's folder, and the button says so.
    a.session.path = Some(at("home/ada/Documents/work.mailcraft"));
    assert_eq!(a.data_folder_shown(false), Some(Shown::Mailbox(at("home/ada/Documents"))));
    assert_eq!(a.execute("ui.openDataFolder", json!({})).unwrap()["opened"], "mailbox");
    assert_eq!(asked.lock().unwrap().last(), Some(&at("home/ada/Documents")));
    // A mailbox in the settings folder (the usual one): the settings folder itself.
    a.session.path = Some(at("home/ada/.config/mailcraft/mailbox.mailcraft"));
    assert_eq!(a.data_folder_shown(false), Some(Shown::Settings(at("home/ada/.config/mailcraft"))));
    assert_eq!(a.execute("ui.openDataFolder", json!({})).unwrap()["opened"], "settings");
    assert_eq!(asked.lock().unwrap().last(), Some(&at("home/ada/.config/mailcraft")));
    // A mailbox in a folder whose name only begins like the settings folder's is elsewhere.
    a.session.path = Some(at("home/ada/.config/mailcraft-old/mailbox.mailcraft"));
    assert_eq!(a.data_folder_shown(false), Some(Shown::Mailbox(at("home/ada/.config/mailcraft-old"))));
    // `mailcraft --mailbox work.mailcraft` (or `sub/x.mailcraft`): relative to where MailCraft
    // was started, so that folder, made whole.
    let here = std::env::current_dir().unwrap();
    a.session.path = Some(PathBuf::from("work.mailcraft"));
    assert_eq!(a.data_folder_shown(false), Some(Shown::Mailbox(here.clone())));
    assert_eq!(a.execute("ui.openDataFolder", json!({})).unwrap()["opened"], "mailbox");
    assert_eq!(asked.lock().unwrap().last(), Some(&here));
    a.session.path = Some(PathBuf::from("sub/x.mailcraft"));
    assert_eq!(a.data_folder_shown(false), Some(Shown::Mailbox(here.join("sub"))));
}

#[test]
fn a_mailbox_really_in_the_settings_folder_is_shown_as_the_settings_folder() {
    // The settings folder spelt another way (`..`) is still the settings folder.
    let (host, store) = (Host::default(), Store::default());
    let mut a = app(&host, &store);
    let settings = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let (folder, asked) = folder_host(settings.clone(), false);
    a.services.data_folder = Some(folder);
    a.session.path = Some(settings.join("..").join("src").join("lib.rs"));
    assert_eq!(a.data_folder_shown(true), Some(Shown::Settings(settings.clone())));
    // The button's label doesn't look at the disk (see `data_folder_shown`): with `..` in the
    // path it can't tell, and says the mailbox's folder; the click settles it. (Windows makes a
    // path whole by folding `..` away, so there the label already knows.)
    let label = a.data_folder_shown(false);
    if cfg!(windows) {
        assert_eq!(label, Some(Shown::Settings(settings.clone())));
    } else {
        assert!(matches!(label, Some(Shown::Mailbox(_))));
    }
    assert_eq!(a.execute("ui.openDataFolder", json!({})).unwrap()["opened"], "settings");
    // And a mailbox beside it is not.
    a.session.path = Some(settings.join("..").join("Cargo.toml"));
    assert!(matches!(a.data_folder_shown(true), Some(Shown::Mailbox(_))));
    assert!(matches!(a.data_folder_shown(false), Some(Shown::Mailbox(_))));
    assert_eq!(asked.lock().unwrap().as_slice(), [settings]);
}

#[test]
fn the_folder_shown_follows_the_mailbox_opened_since_start() {
    let (host, store) = (Host::default(), Store::default());
    let mut a = app(&host, &store);
    let (folder, asked) = folder_host(at("home/ada/.config/mailcraft"), false);
    a.services.data_folder = Some(folder);
    a.session.path = Some(at("home/ada/.config/mailcraft/mailbox.mailcraft"));
    a.execute("ui.openDataFolder", json!({})).unwrap();
    // File › Open of another mailbox changes where the open mailbox is: decided at the click.
    a.session.path = Some(at("media/usb/archive/2019.mailcraft"));
    a.execute("ui.openDataFolder", json!({})).unwrap();
    // And back to one in the settings folder.
    a.session.path = Some(at("home/ada/.config/mailcraft/mailbox.mailcraft"));
    a.execute("ui.openDataFolder", json!({})).unwrap();
    assert_eq!(asked.lock().unwrap().as_slice(), [at("home/ada/.config/mailcraft"), at("media/usb/archive"), at("home/ada/.config/mailcraft")]);
}
