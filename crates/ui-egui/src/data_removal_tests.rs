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
