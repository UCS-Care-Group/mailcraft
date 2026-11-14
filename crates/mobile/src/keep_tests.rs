// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The phone's mailbox on disk: it survives a restart, is written before a send, keeps the last
//! good file when a save fails, sets an unreadable file aside, and goes when the owner removes
//! the data. Synthetic mail (`example.org`) in a directory of the test's own.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mailcraft_engine::Session;
use mailcraft_host::graph::{GraphRequest, GraphResult};
use mailcraft_host::secrets::secret_key;
use mailcraft_host::sync::SendReceive;
use mailcraft_model::{Address, FolderKind, Message};
use serde_json::json;

use crate::fake_tests::{FakePlatform, block_on};
use crate::{Core, CoreError, DataRemoved, Event, StorageNotice};

/// A directory of this test's own, removed afterwards.
struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!("mc-keep-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&p);
        Dir(p)
    }

    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }

    fn file(&self) -> PathBuf {
        self.0.join("mailbox.mailcraft")
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        restore_access(&self.0);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
fn set_mode(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode));
}

#[cfg(unix)]
fn walk(p: &Path, mode: u32) {
    if let Ok(entries) = std::fs::read_dir(p) {
        for e in entries.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                walk(&e.path(), mode);
            }
        }
    }
    set_mode(p, mode);
}

fn restore_access(p: &Path) {
    #[cfg(unix)]
    walk(p, 0o700);
    #[cfg(not(unix))]
    let _ = p;
}

fn open(dir: &Dir) -> Arc<Core> {
    Core::open(FakePlatform::new(), dir.path().to_string()).unwrap()
}

fn drain(core: &Core) -> Vec<Event> {
    let mut out = Vec::new();
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        let mut next = std::pin::pin!(core.next_event());
        match next.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(Some(e)) => out.push(e),
            _ => return out,
        }
    }
}

/// A body past what the file keeps inline, so that it is written to the content folder.
fn big(tag: &str) -> String {
    format!("{tag} {}", "synthetic text ".repeat(200))
}

/// Adds a Microsoft account with a message in the Inbox, a draft and one in the Outbox that is
/// marked as sending; gives the account.
fn fill(core: &Core) -> u64 {
    block_on(core.call(|s| {
        let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        let mut add = |kind: FolderKind, subject: &str, sending: bool, body: String| {
            let folder = s.mailbox.folder_of_kind(account, kind).unwrap();
            let mut m = Message {
                account,
                folder,
                subject: subject.into(),
                to: vec![Address::new("", "kit@example.net")],
                message_id: format!("{}@example.org", subject.to_lowercase().replace(' ', "-")),
                sending,
                ..Default::default()
            };
            m.body.text = body;
            s.mailbox.add_message(m).unwrap();
        };
        add(FolderKind::Inbox, "Budget review", false, big("inbox"));
        add(FolderKind::Drafts, "Half written", false, "dear kit,".into());
        add(FolderKind::Outbox, "Going out", true, "on its way".into());
        s.mailbox_changed();
        Ok(account)
    }))
    .unwrap()
}

fn subjects(core: &Core) -> Vec<(String, bool)> {
    let mut v: Vec<_> = block_on(core.call(|s| Ok(s.mailbox.messages.values().map(|m| (m.subject.clone(), m.sending)).collect::<Vec<_>>()))).unwrap();
    v.sort();
    v
}

fn on_disk(path: &Path) -> Vec<(String, bool)> {
    let mut v: Vec<_> = mailcraft_engine::store::open(path).unwrap().mailbox.messages.values().map(|m| (m.subject.clone(), m.sending)).collect();
    v.sort();
    v
}

fn until(what: &str, f: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < end, "never happened: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn mail_a_draft_and_a_sending_mark_survive_a_restart() {
    let dir = Dir::new();
    {
        let core = open(&dir);
        fill(&core);
        block_on(core.save_now()).unwrap();
        // The process ends (Android kills it): no more than the core's own end is given.
        core.shutdown();
    }
    let core = open(&dir);
    let kept = subjects(&core);
    assert_eq!(kept, [("Budget review".into(), false), ("Going out".into(), true), ("Half written".into(), false)]);
    // The body that went to the content folder is read back on demand.
    let text = block_on(core.call(|s| {
        let id = *s.mailbox.messages.iter().find(|(_, m)| m.subject == "Budget review").unwrap().0;
        Ok(s.loaded_message(id).unwrap().body.text.clone())
    }))
    .unwrap();
    assert!(text.starts_with("inbox synthetic"), "{text}");
    assert!(drain(&core).iter().all(|e| !matches!(e, Event::StorageNotice { .. })));
}

#[test]
fn a_change_is_written_by_itself_and_the_last_state_when_the_core_ends() {
    let dir = Dir::new();
    let core = open(&dir);
    fill(&core);
    until("the autosave", || dir.file().exists() && on_disk(&dir.file()).len() == 3);
    // A change just before the end has no time for the autosave: the end writes it.
    block_on(core.call(|s| {
        s.run("account.add", &json!({"email": "second@example.org", "kind": "microsoft"})).unwrap();
        Ok(())
    }))
    .unwrap();
    core.shutdown();
    let again = open(&dir);
    let emails: Vec<String> = block_on(again.call(|s| Ok(s.mailbox.accounts.iter().map(|a| a.email.clone()).collect()))).unwrap();
    assert!(emails.contains(&"second@example.org".to_string()), "{emails:?}");
}

#[test]
fn the_sending_mark_is_on_disk_before_the_send_job_starts() {
    let dir = Dir::new();
    let file = dir.file();
    let seen: Arc<Mutex<Vec<Vec<(String, bool)>>>> = Arc::default();
    let log = seen.clone();
    let runner = Arc::new(move |req: GraphRequest| {
        // What the file holds the moment the run begins.
        if !req.outbox.is_empty() || req.mail.as_ref().is_some_and(|m| !m.sends.is_empty()) {
            log.lock().unwrap().push(on_disk(&file));
        }
        GraphResult { account: req.account.id, ..Default::default() }
    });
    let platform = FakePlatform::new();
    let core = Core::start_stored(
        platform.clone(),
        |keep| {
            let mut s = Session::new(keep.unwrap().open());
            s.prefs.sync_minutes = 0;
            s
        },
        SendReceive::with_runner(move |req| runner(req)),
        Some(dir.path().to_string()),
    )
    .unwrap();
    let account = fill(&core);
    let key = block_on(core.call(move |s| Ok(secret_key("refresh", s.mailbox.account(account).unwrap())))).unwrap();
    platform.secrets.kept.lock().unwrap().push((key, "stored-refresh".to_string()));
    block_on(core.call(|s| {
        // The message waits unmarked: the run marks it, and the file must hear of it first.
        for m in s.mailbox.messages.values_mut().filter(|m| m.subject == "Going out") {
            std::sync::Arc::make_mut(m).sending = false;
        }
        Ok(())
    }))
    .unwrap();
    assert!(block_on(core.sync_now()).unwrap().started);
    until("the send run", || !seen.lock().unwrap().is_empty());
    let first = seen.lock().unwrap()[0].clone();
    assert!(first.contains(&("Going out".to_string(), true)), "{first:?}");
}

#[cfg(unix)]
#[test]
fn a_failed_save_keeps_the_last_good_file_and_says_so() {
    let dir = Dir::new();
    let core = open(&dir);
    fill(&core);
    block_on(core.save_now()).unwrap();
    let good = on_disk(&dir.file());
    assert_eq!(good.len(), 3);
    // The content folder can't be written to any more (a full or read-only disk).
    let blobs = dir.0.join("mailbox.mailcraft.blobs");
    walk(&blobs, 0o500);
    if std::fs::write(blobs.join("probe"), b"x").is_ok() {
        // (Running with rights that ignore it: nothing to show.)
        return;
    }
    block_on(core.call(|s| {
        let folder = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Inbox).unwrap();
        let account = s.mailbox.accounts[0].id;
        let mut m = Message { account, folder, subject: "Too late".into(), message_id: "late@example.org".into(), ..Default::default() };
        m.body.text = big("late");
        s.mailbox.add_message(m).unwrap();
        s.mailbox_changed();
        Ok(())
    }))
    .unwrap();
    let e = block_on(core.save_now()).unwrap_err();
    assert!(matches!(e, CoreError::Failed { .. }), "{e:?}");
    assert!(drain(&core).iter().any(|e| matches!(e, Event::SaveFailed { .. })));
    assert_eq!(on_disk(&dir.file()), good, "the last good file stands");
    // The disk works again: the next save writes it, nothing was lost meanwhile.
    walk(&blobs, 0o700);
    block_on(core.save_now()).unwrap();
    assert_eq!(on_disk(&dir.file()).len(), 4);
}

#[test]
fn an_unreadable_file_is_set_aside_and_a_fresh_mailbox_opens() {
    let dir = Dir::new();
    std::fs::create_dir_all(&dir.0).unwrap();
    let junk = b"this is not a mailbox file at all, however it is dressed up".repeat(8);
    std::fs::write(dir.file(), &junk).unwrap();
    let core = open(&dir);
    let events = drain(&core);
    let Some(Event::StorageNotice { notice: StorageNotice::SetAside, detail }) = events.iter().find(|e| matches!(e, Event::StorageNotice { .. }))
    else {
        panic!("no notice: {events:?}");
    };
    assert!(detail.starts_with("mailbox.mailcraft.unreadable-"), "{detail}");
    assert_eq!(std::fs::read(dir.0.join(detail)).unwrap(), junk, "kept as it was");
    assert!(!dir.file().exists());
    assert!(subjects(&core).is_empty());
    // It is a working mailbox: what is added is kept, and the kept-aside file is untouched.
    fill(&core);
    block_on(core.save_now()).unwrap();
    assert_eq!(on_disk(&dir.file()).len(), 3);
    assert_eq!(std::fs::read(dir.0.join(detail)).unwrap(), junk);
}

#[cfg(unix)]
#[test]
fn a_file_that_cannot_be_kept_aside_is_never_written_over() {
    let dir = Dir::new();
    std::fs::create_dir_all(&dir.0).unwrap();
    let junk = b"this is not a mailbox file at all, however it is dressed up".repeat(8);
    std::fs::write(dir.file(), &junk).unwrap();
    // A folder that can't be changed: the rename fails.
    set_mode(&dir.0, 0o500);
    if std::fs::write(dir.0.join("probe"), b"x").is_ok() {
        return;
    }
    let core = open(&dir);
    assert!(drain(&core).iter().any(|e| matches!(e, Event::StorageNotice { notice: StorageNotice::NotKept, .. })));
    fill(&core);
    assert!(block_on(core.save_now()).is_err());
    assert_eq!(std::fs::read(dir.file()).unwrap(), junk);
}

#[test]
fn removing_the_data_removes_the_files_and_the_sign_ins() {
    let dir = Dir::new();
    let platform = FakePlatform::new();
    let core = Core::open(platform.clone(), dir.path().to_string()).unwrap();
    let account = fill(&core);
    let key = block_on(core.call(move |s| Ok(secret_key("refresh", s.mailbox.account(account).unwrap())))).unwrap();
    platform.secrets.kept.lock().unwrap().push((key, "stored-refresh".to_string()));
    block_on(core.save_now()).unwrap();
    std::fs::write(dir.0.join("mailbox.mailcraft.unreadable-1"), b"old").unwrap();
    assert!(dir.0.join("mailbox.mailcraft.blobs").exists());
    block_on(core.delete_local_data()).unwrap();
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
    assert!(platform.secrets.kept.lock().unwrap().is_empty());
    assert!(subjects(&core).is_empty());
    // Nothing comes back by itself, and a new change is kept again.
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
    fill(&core);
    block_on(core.save_now()).unwrap();
    assert_eq!(on_disk(&dir.file()).len(), 3);
}

/// A core with a mailbox, its sign-in kept, and the platform that holds it.
fn with_stored_sign_in(dir: &Dir) -> (Arc<Core>, Arc<FakePlatform>) {
    let platform = FakePlatform::new();
    let core = Core::open(platform.clone(), dir.path().to_string()).unwrap();
    let account = fill(&core);
    let key = block_on(core.call(move |s| Ok(secret_key("refresh", s.mailbox.account(account).unwrap())))).unwrap();
    platform.secrets.kept.lock().unwrap().push((key, "stored-refresh".to_string()));
    block_on(core.save_now()).unwrap();
    (core, platform)
}

#[test]
fn removing_the_data_says_so_when_a_sign_in_stays_in_the_store_and_a_retry_removes_it() {
    let dir = Dir::new();
    let (core, platform) = with_stored_sign_in(&dir);
    platform.secrets.stuck.store(true, Ordering::Relaxed);
    assert!(matches!(block_on(core.delete_local_data()), Err(CoreError::Failed { .. })));
    assert!(platform.secrets.kept.lock().unwrap().iter().any(|(_, v)| v == "stored-refresh"), "the token is still there, and the owner was told");
    assert!(subjects(&core).is_empty(), "the mail's files went, so the session let go of it too");
    platform.secrets.stuck.store(false, Ordering::Relaxed);
    block_on(core.delete_local_data()).unwrap();
    assert!(platform.secrets.kept.lock().unwrap().is_empty());
    assert!(subjects(&core).is_empty());
}

/// Adds one message to the Inbox of the mailbox's first account.
fn add_one(core: &Core, subject: &'static str) {
    block_on(core.call(move |s| {
        let account = s.mailbox.accounts.first().map(|a| a.id).unwrap();
        let folder = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
        let m = Message { account, folder, subject: subject.into(), message_id: "after@example.org".into(), ..Default::default() };
        s.mailbox.add_message(m).unwrap();
        s.mailbox_changed();
        Ok(())
    }))
    .unwrap();
}

/// After a removal that failed, the old mail never comes back by itself, and a new change is kept
/// without it.
fn nothing_old_comes_back(core: &Core, dir: &Dir) {
    assert!(subjects(core).is_empty(), "the session let go of the mail");
    std::thread::sleep(Duration::from_millis(400));
    assert!(!dir.file().exists(), "no save brought the mailbox back");
    add_one(core, "After the removal");
    until("the change is saved", || dir.file().exists());
    block_on(core.save_now()).unwrap();
    assert_eq!(on_disk(&dir.file()), vec![("After the removal".to_string(), false)]);
}

#[test]
fn a_removal_whose_sign_ins_stay_lets_go_of_the_mail_it_deleted() {
    let dir = Dir::new();
    let (core, platform) = with_stored_sign_in(&dir);
    platform.secrets.stuck.store(true, Ordering::Relaxed);
    assert!(matches!(block_on(core.delete_local_data()), Err(CoreError::Failed { .. })));
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0, "the mail's files went");
    nothing_old_comes_back(&core, &dir);
    // The retry still removes the sign-in it couldn't before.
    platform.secrets.stuck.store(false, Ordering::Relaxed);
    block_on(core.delete_local_data()).unwrap();
    assert!(platform.secrets.kept.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn a_removal_where_both_fail_but_some_files_went_lets_go_of_the_mail() {
    let dir = Dir::new();
    let (core, platform) = with_stored_sign_in(&dir);
    platform.secrets.stuck.store(true, Ordering::Relaxed);
    // The mailbox file can go, but not what the content folder holds.
    let blobs = dir.0.join("mailbox.mailcraft.blobs");
    assert!(blobs.exists());
    set_mode(&blobs, 0o500);
    let probe = blobs.join("probe");
    if std::fs::write(&probe, b"x").is_ok() {
        // (Run with rights that ignore the mode: nothing to see here.)
        restore_access(&dir.0);
        return;
    }
    assert!(matches!(block_on(core.delete_local_data()), Err(CoreError::Failed { .. })));
    assert!(!dir.file().exists() && blobs.exists(), "some went and some stayed");
    nothing_old_comes_back(&core, &dir);
    restore_access(&dir.0);
    platform.secrets.stuck.store(false, Ordering::Relaxed);
    block_on(core.delete_local_data()).unwrap();
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
    assert!(platform.secrets.kept.lock().unwrap().is_empty());
}

#[test]
fn removing_the_data_says_so_when_the_credential_store_cannot_be_reached() {
    let dir = Dir::new();
    let (core, platform) = with_stored_sign_in(&dir);
    platform.no_store.store(true, Ordering::Relaxed);
    assert!(matches!(block_on(core.delete_local_data()), Err(CoreError::Failed { .. })));
    platform.no_store.store(false, Ordering::Relaxed);
    block_on(core.delete_local_data()).unwrap();
    assert!(platform.secrets.kept.lock().unwrap().is_empty());
}

#[test]
fn a_sign_in_that_cannot_be_read_back_is_not_taken_for_removed() {
    let dir = Dir::new();
    let (core, platform) = with_stored_sign_in(&dir);
    // The removal fails, and so does the read that would show it is still there.
    platform.secrets.stuck.store(true, Ordering::Relaxed);
    platform.secrets.unreadable.store(true, Ordering::Relaxed);
    assert!(matches!(block_on(core.delete_local_data()), Err(CoreError::Failed { .. })));
    assert!(platform.secrets.kept.lock().unwrap().iter().any(|(_, v)| v == "stored-refresh"));
    assert!(subjects(&core).is_empty(), "the mail's files went, so the session let go of it too");
    platform.secrets.stuck.store(false, Ordering::Relaxed);
    platform.secrets.unreadable.store(false, Ordering::Relaxed);
    block_on(core.delete_local_data()).unwrap();
    assert!(platform.secrets.kept.lock().unwrap().is_empty());
}

#[test]
fn a_restarted_core_keeps_an_index_it_could_not_read_and_a_retry_removes_what_it_names() {
    let dir = Dir::new();
    let platform = FakePlatform::new();
    // An earlier run kept a sign-in for an account the mailbox no longer has, and named it in
    // the index; this run starts with the store out of reach.
    let gone = "refresh:gone@example.org|graph".to_string();
    platform.secrets.kept.lock().unwrap().push((gone.clone(), "old-refresh".to_string()));
    platform.secrets.kept.lock().unwrap().push(("mailcraft:index".to_string(), serde_json::to_string(&[&gone]).unwrap()));
    platform.no_store.store(true, Ordering::Relaxed);
    let core = Core::open(platform.clone(), dir.path().to_string()).unwrap();
    fill(&core);
    assert!(matches!(block_on(core.delete_local_data()), Err(CoreError::Failed { .. })));
    platform.no_store.store(false, Ordering::Relaxed);
    block_on(core.delete_local_data()).unwrap();
    assert!(platform.secrets.kept.lock().unwrap().is_empty(), "{:?}", platform.secrets.kept.lock().unwrap());
}

#[test]
fn an_index_that_does_not_parse_is_set_aside_and_never_taken_for_an_empty_one() {
    let dir = Dir::new();
    let platform = FakePlatform::new();
    // An earlier run kept a sign-in for an account the mailbox no longer has; the index that
    // named it is now garbage.
    let gone = "refresh:gone@example.org|graph".to_string();
    platform.secrets.kept.lock().unwrap().push((gone.clone(), "old-refresh".to_string()));
    platform.secrets.kept.lock().unwrap().push(("mailcraft:index".to_string(), "{not a list".to_string()));
    let core = Core::open(platform.clone(), dir.path().to_string()).unwrap();
    fill(&core);
    // The removal goes through, and says that older sign-ins may be left.
    let answer = block_on(core.delete_local_data()).unwrap();
    assert!(matches!(&answer, DataRemoved::SignInsMayRemain { message } if message.contains("Settings") && !message.contains("mail")), "{answer:?}");
    assert!(subjects(&core).is_empty(), "the mail is removed");
    let kept = platform.secrets.kept.lock().unwrap().clone();
    // The garbage is set aside, not lost, and nothing parses as the index any more.
    assert!(kept.iter().any(|(k, v)| k.starts_with("mailcraft:index.unreadable-") && v == "{not a list"), "set aside: {kept:?}");
    assert!(!kept.iter().any(|(k, _)| k == "mailcraft:index"), "the fresh index is removed with the rest: {kept:?}");
    // The sign-in only the garbage named can't be found: it is what the message is about.
    assert!(kept.iter().any(|(k, _)| *k == gone), "{kept:?}");
    // Until the app says it was shown, every removal says it again.
    assert!(matches!(block_on(core.delete_local_data()).unwrap(), DataRemoved::SignInsMayRemain { .. }));
    // Once shown, the next removal finds a sound (absent) index and says nothing more.
    block_on(core.sign_ins_notice_shown()).unwrap();
    assert_eq!(block_on(core.delete_local_data()).unwrap(), DataRemoved::All);
}

/// A store whose index is garbage and that still holds the sign-in of an account the mailbox no
/// longer has, under a core with a mailbox of its own.
fn with_damaged_index(dir: &Dir) -> (Arc<Core>, Arc<FakePlatform>) {
    let platform = FakePlatform::new();
    let gone = "refresh:gone@example.org|graph".to_string();
    platform.secrets.kept.lock().unwrap().push((gone, "old-refresh".to_string()));
    platform.secrets.kept.lock().unwrap().push(("mailcraft:index".to_string(), "{not a list".to_string()));
    let core = Core::open(platform.clone(), dir.path().to_string()).unwrap();
    fill(&core);
    (core, platform)
}

#[test]
fn a_lost_answer_about_a_damaged_index_is_given_again_until_it_was_shown() {
    // The first answer sets the damaged index aside; if it is lost on its way to the window,
    // the retry finds a sound index but must still say that older sign-ins may remain.
    let dir = Dir::new();
    let (core, platform) = with_damaged_index(&dir);
    let first = block_on(core.forget_sign_ins()).unwrap();
    assert!(first.may_remain.is_some(), "{first:?}");
    assert!(platform.secrets.kept.lock().unwrap().iter().any(|(k, _)| k.starts_with("mailcraft:index.unreadable-")), "set aside on the first call");
    let retry = block_on(core.forget_sign_ins()).unwrap();
    assert!(retry.may_remain.is_some(), "the retry still says it: {retry:?}");
    block_on(core.sign_ins_notice_shown()).unwrap();
    let after = block_on(core.forget_sign_ins()).unwrap();
    assert_eq!(after.may_remain, None, "{after:?}");

    // The same through "remove my data".
    let dir = Dir::new();
    let (core, _platform) = with_damaged_index(&dir);
    assert!(matches!(block_on(core.delete_local_data()).unwrap(), DataRemoved::SignInsMayRemain { .. }));
    assert!(matches!(block_on(core.delete_local_data()).unwrap(), DataRemoved::SignInsMayRemain { .. }), "the retry still says it");
    block_on(core.sign_ins_notice_shown()).unwrap();
    assert_eq!(block_on(core.delete_local_data()).unwrap(), DataRemoved::All);
}

#[test]
fn an_index_that_does_not_parse_starts_a_fresh_one_with_the_names_known_now() {
    let dir = Dir::new();
    let (core, platform) = with_stored_sign_in(&dir);
    let index = |platform: &FakePlatform| platform.secrets.kept.lock().unwrap().iter().find(|(k, _)| k == "mailcraft:index").map(|(_, v)| v.clone());
    platform.secrets.kept.lock().unwrap().push(("mailcraft:index".to_string(), "[not, json".to_string()));
    // The account's sign-in can't be removed this time: the fresh index names it, so a retry
    // finds it, and the notice waits for the removal that succeeds.
    platform.secrets.stuck.store(true, Ordering::Relaxed);
    assert!(matches!(block_on(core.delete_local_data()), Err(CoreError::Failed { .. })));
    let fresh = index(&platform).unwrap_or_default();
    let names: Vec<String> = serde_json::from_str(&fresh).unwrap();
    assert!(names.iter().any(|n| n.starts_with("refresh:")), "{fresh}");
    assert!(platform.secrets.kept.lock().unwrap().iter().any(|(k, v)| k.starts_with("mailcraft:index.unreadable-") && v == "[not, json"));
    platform.secrets.stuck.store(false, Ordering::Relaxed);
    assert!(matches!(block_on(core.delete_local_data()).unwrap(), DataRemoved::SignInsMayRemain { .. }));
    block_on(core.sign_ins_notice_shown()).unwrap();
    let kept = platform.secrets.kept.lock().unwrap().clone();
    assert!(kept.iter().all(|(k, _)| k.starts_with("mailcraft:index.unreadable-")), "only the set-aside garbage is left: {kept:?}");
}

#[test]
fn an_index_that_does_not_parse_and_cannot_be_set_aside_is_kept_and_the_rest_removed() {
    let dir = Dir::new();
    let (core, platform) = with_stored_sign_in(&dir);
    platform.secrets.kept.lock().unwrap().push(("mailcraft:index".to_string(), "{not a list".to_string()));
    // The store refuses another name beside the index (and the index itself).
    platform.secrets.fail_index.store(usize::MAX, Ordering::Relaxed);
    let answer = block_on(core.delete_local_data()).unwrap();
    assert!(matches!(answer, DataRemoved::SignInsMayRemain { .. }), "{answer:?}");
    block_on(core.sign_ins_notice_shown()).unwrap();
    let kept = platform.secrets.kept.lock().unwrap().clone();
    assert_eq!(kept, vec![("mailcraft:index".to_string(), "{not a list".to_string())], "the garbage is kept as it was, the sign-in removed");
}

#[test]
fn an_index_the_store_cannot_read_is_kept_and_the_removal_refused() {
    // Unchanged by the setting aside: a read error is not garbage.
    let dir = Dir::new();
    let (core, platform) = with_stored_sign_in(&dir);
    platform.secrets.kept.lock().unwrap().push(("mailcraft:index".to_string(), "{not a list".to_string()));
    platform.no_store.store(true, Ordering::Relaxed);
    assert!(matches!(block_on(core.delete_local_data()), Err(CoreError::Failed { .. })));
    platform.no_store.store(false, Ordering::Relaxed);
    let kept = platform.secrets.kept.lock().unwrap().clone();
    assert!(kept.iter().any(|(k, _)| k == "mailcraft:index") && !kept.iter().any(|(k, _)| k.contains("unreadable")), "{kept:?}");
}

#[cfg(unix)]
#[test]
fn the_notice_of_a_damaged_index_waits_for_a_removal_that_succeeds() {
    let dir = Dir::new();
    let (core, platform) = with_stored_sign_in(&dir);
    platform.secrets.kept.lock().unwrap().push(("mailcraft:index".to_string(), "{not a list".to_string()));
    // The sign-ins go, but the mail's folder can't be emptied.
    set_mode(&dir.0, 0o000);
    if std::fs::read_dir(&dir.0).is_ok() {
        restore_access(&dir.0);
        return;
    }
    assert!(matches!(block_on(core.delete_local_data()), Err(CoreError::Failed { .. })));
    restore_access(&dir.0);
    let answer = block_on(core.delete_local_data()).unwrap();
    assert!(matches!(answer, DataRemoved::SignInsMayRemain { .. }), "the retry still says it: {answer:?}");
    block_on(core.sign_ins_notice_shown()).unwrap();
    assert_eq!(block_on(core.delete_local_data()).unwrap(), DataRemoved::All, "and says it once it was shown");
}

#[cfg(unix)]
#[test]
fn the_notice_of_a_damaged_index_outlives_a_restart_before_the_retry() {
    let dir = Dir::new();
    let (core, platform) = with_stored_sign_in(&dir);
    platform.secrets.kept.lock().unwrap().push(("mailcraft:index".to_string(), "{not a list".to_string()));
    // The sign-ins go (the damaged index with them), but the mail's folder can't be emptied.
    set_mode(&dir.0, 0o000);
    if std::fs::read_dir(&dir.0).is_ok() {
        restore_access(&dir.0);
        return;
    }
    assert!(matches!(block_on(core.delete_local_data()), Err(CoreError::Failed { .. })));
    // The app ends before the owner tries again.
    core.shutdown();
    drop(core);
    restore_access(&dir.0);
    let core = Core::open(platform.clone(), dir.path().to_string()).unwrap();
    let answer = block_on(core.delete_local_data()).unwrap();
    assert!(matches!(answer, DataRemoved::SignInsMayRemain { .. }), "the retry after the restart still says it: {answer:?}");
    block_on(core.sign_ins_notice_shown()).unwrap();
    let kept = platform.secrets.kept.lock().unwrap().clone();
    assert!(
        kept.iter().all(|(k, _)| k.starts_with("mailcraft:index.unreadable-")),
        "once shown, nothing of it is left but the set-aside garbage: {kept:?}"
    );
    assert_eq!(block_on(core.delete_local_data()).unwrap(), DataRemoved::All, "and says it once it was shown");
}

#[test]
fn a_damaged_index_is_kept_as_it_is_when_its_notice_cannot_be_kept() {
    let dir = Dir::new();
    let (core, platform) = with_stored_sign_in(&dir);
    platform.secrets.kept.lock().unwrap().push(("mailcraft:index".to_string(), "{not a list".to_string()));
    platform.secrets.notice_unwritable.store(true, Ordering::Relaxed);
    let answer = block_on(core.delete_local_data()).unwrap();
    assert!(matches!(answer, DataRemoved::SignInsMayRemain { .. }), "{answer:?}");
    let kept = platform.secrets.kept.lock().unwrap().clone();
    // Nothing was set aside: the damaged index itself is what a later run finds again.
    assert_eq!(kept, vec![("mailcraft:index".to_string(), "{not a list".to_string())]);
}

#[test]
fn a_removal_is_not_taken_for_complete_while_a_notice_still_due_cannot_be_read() {
    let dir = Dir::new();
    let platform = FakePlatform::new();
    // An earlier run found the index damaged and couldn't say so before it ended.
    platform.secrets.kept.lock().unwrap().push(("mailcraft:notice-sign-ins-may-remain".to_string(), "1".to_string()));
    platform.secrets.notice_unreadable.store(true, Ordering::Relaxed);
    let core = Core::open(platform.clone(), dir.path().to_string()).unwrap();
    assert!(matches!(block_on(core.delete_local_data()), Err(CoreError::Failed { .. })));
    platform.secrets.notice_unreadable.store(false, Ordering::Relaxed);
    assert!(matches!(block_on(core.delete_local_data()).unwrap(), DataRemoved::SignInsMayRemain { .. }));
    block_on(core.sign_ins_notice_shown()).unwrap();
    assert!(platform.secrets.kept.lock().unwrap().is_empty());
    assert_eq!(block_on(core.delete_local_data()).unwrap(), DataRemoved::All, "and says it once it was shown");
}

#[cfg(unix)]
#[test]
fn a_file_that_cannot_be_looked_at_is_never_written_over() {
    let dir = Dir::new();
    {
        let core = open(&dir);
        fill(&core);
        block_on(core.save_now()).unwrap();
        core.shutdown();
    }
    let before = std::fs::read(dir.file()).unwrap();
    // A folder that can't be entered: the file can neither be looked at nor read.
    set_mode(&dir.0, 0o000);
    if std::fs::metadata(dir.file()).is_ok() {
        restore_access(&dir.0);
        return;
    }
    let core = open(&dir);
    assert!(drain(&core).iter().any(|e| matches!(e, Event::StorageNotice { notice: StorageNotice::NotKept, .. })));
    fill(&core);
    assert!(block_on(core.save_now()).is_err());
    restore_access(&dir.0);
    assert_eq!(std::fs::read(dir.file()).unwrap(), before);
}

#[test]
fn junk_directories_never_panic() {
    let dir = Dir::new();
    std::fs::create_dir_all(&dir.0).unwrap();
    let a_file = dir.0.join("plain");
    std::fs::write(&a_file, b"x").unwrap();
    let long = "/".to_string() + &"a".repeat(5000);
    let inside_file = format!("{}/inside", a_file.display());
    for bad in ["", "relative/dir", ".", "/a\0b", long.as_str(), a_file.to_str().unwrap(), inside_file.as_str(), "/proc/nope/never"] {
        let r = Core::open(FakePlatform::new(), bad.to_string());
        assert!(matches!(r, Err(CoreError::Init { .. })), "{bad:?}");
    }
    // A core with no folder has nothing to write: saving is a no-op, removing the data too.
    let plain = Core::start(FakePlatform::new(), Session::empty).unwrap();
    block_on(plain.save_now()).unwrap();
    block_on(plain.delete_local_data()).unwrap();
}
