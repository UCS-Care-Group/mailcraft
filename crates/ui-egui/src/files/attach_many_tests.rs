// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Attach File takes several files at once: the dialog lets the owner pick them, and each is
//! attached in the order the dialog lists them. Every other dialog still picks one file. What is
//! read of a pick is decided from the files' sizes before any is read ([`super::reads_of_pick`]).

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use serde_json::json;

use super::FileDialogRequest;
use super::{MOST_ATTACHED, MOST_ATTACHED_BYTES, MOST_ATTACHMENT_BYTES, reads_of_pick};
use crate::{Delivered, MailApp, Services, Unread};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// A message being written, with a file dialog that answers `picked` at once.
fn writing(picked: Option<Vec<String>>) -> (MailApp, Arc<Mutex<Vec<FileDialogRequest>>>) {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = asked.clone();
    app.services.file_dialog = Some(Box::new(move |request| {
        seen.lock().unwrap().push(request);
        let (tx, rx) = mpsc::channel();
        tx.send(picked.clone()).unwrap();
        rx
    }));
    app.run("mail.new", json!({"to": "kit@example.org", "subject": "Figures"})).unwrap();
    (app, asked)
}

/// Files of a folder of their own for one test, removed when it ends.
struct Files(std::path::PathBuf);

impl Files {
    fn new(test: &str, names: &[&str]) -> (Self, Vec<String>) {
        let dir = std::env::temp_dir().join(format!("mailcraft-attach-many-{test}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths = names
            .iter()
            .map(|name| {
                let path = dir.join(name);
                std::fs::write(&path, format!("the text of {name}")).unwrap();
                path.to_string_lossy().to_string()
            })
            .collect();
        (Files(dir), paths)
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn attached(app: &MailApp) -> Vec<String> {
    app.session.compose.as_ref().unwrap().attachments.iter().map(|a| a.name.clone()).collect()
}

fn said(app: &MailApp) -> String {
    app.status_msg.clone().unwrap_or_default().0
}

#[test]
fn only_files_to_attach_are_picked_several_at_once() {
    assert!(FileDialogRequest::Open { purpose: "attach".into() }.picks_many());
    for purpose in ["mailbox", "import", "picture", "bodyPicture"] {
        assert!(!FileDialogRequest::Open { purpose: purpose.into() }.picks_many(), "{purpose}");
    }
    assert!(!FileDialogRequest::Folder.picks_many());
    assert!(!FileDialogRequest::Save { name: "a.eml".into() }.picks_many());
}

#[test]
fn every_file_picked_is_attached_in_the_order_the_dialog_lists_them() {
    let (_files, paths) = Files::new("all", &["minutes.txt", "budget.csv", "notes.eml"]);
    let (mut app, asked) = writing(Some(paths));
    app.run("ui.attach", json!({})).unwrap();
    assert_eq!(*asked.lock().unwrap(), [FileDialogRequest::Open { purpose: "attach".into() }]);
    // A message file among them is attached as the others are, not imported.
    assert_eq!(attached(&app), ["minutes.txt", "budget.csv", "notes.eml"]);
}

#[test]
fn a_file_that_cant_be_attached_doesnt_keep_the_others_out() {
    let (files, mut paths) = Files::new("one-gone", &["minutes.txt", "budget.csv"]);
    paths.insert(1, files.0.join("not-there.txt").to_string_lossy().to_string());
    let (mut app, _) = writing(Some(paths));
    app.run("ui.attach", json!({})).unwrap();
    assert_eq!(attached(&app), ["minutes.txt", "budget.csv"]);
    // The status bar says how many went in and why one didn't, in one line.
    let said = said(&app);
    assert!(said.starts_with("Attached 2 files · ") && said.contains("not-there.txt"), "{said}");
}

/// None could be attached: the reason, and an error.
#[test]
fn nothing_attached_is_an_error_with_the_reason() {
    let (files, _) = Files::new("none", &[]);
    let paths = ["a", "b"].map(|n| files.0.join(format!("{n}.txt")).to_string_lossy().to_string()).to_vec();
    let (mut app, _) = writing(Some(paths));
    // (`ui.attach` answers that the pick is pending; what came of it is the status bar's.)
    app.run("ui.attach", json!({})).unwrap();
    assert!(attached(&app).is_empty());
    assert!(!said(&app).starts_with("Attached") && said(&app).contains("a.txt"), "{}", said(&app));
}

/// A stuck key picks a great many: the first hundred go in, and the status bar says the rest
/// were left out.
#[test]
fn more_than_the_most_at_a_time_are_left_out_and_said() {
    let names: Vec<String> = (0..103).map(|i| format!("file-{i:03}.txt")).collect();
    let (_files, paths) = Files::new("many", &names.iter().map(String::as_str).collect::<Vec<_>>());
    let (mut app, _) = writing(Some(paths));
    app.run("ui.attach", json!({})).unwrap();
    assert_eq!(attached(&app).len(), 100);
    assert_eq!(attached(&app)[99], "file-099.txt");
    assert_eq!(said(&app), "Attached 100 files · 3 more were left out: at most 100 files at a time");
}

#[test]
fn one_file_picked_is_attached_as_before() {
    let (_files, paths) = Files::new("one", &["minutes.txt"]);
    let (mut app, _) = writing(Some(paths));
    app.run("ui.attach", json!({})).unwrap();
    assert_eq!(attached(&app), ["minutes.txt"]);
}

#[test]
fn a_cancelled_pick_attaches_nothing() {
    let (mut app, _) = writing(None);
    app.run("ui.attach", json!({})).unwrap();
    assert!(attached(&app).is_empty());
}

/// What the picker was last opened for, for which message, and which pick that is.
type Opened = Arc<Mutex<Option<(String, Option<u64>, u64)>>>;

/// A browser whose picker answers through `picked`, and what it was last opened for.
fn browser() -> (MailApp, crate::Inbox, Opened) {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    let inbox: crate::Inbox = Arc::new(Mutex::new(Vec::new()));
    app.services.picked = Some(inbox.clone());
    app.services.inbox = Some(Arc::new(Mutex::new(Vec::new())));
    let opened: Opened = Arc::new(Mutex::new(None));
    let noted = opened.clone();
    app.services.open_async = Some(Box::new(move |purpose, asking| {
        let mut noted = noted.lock().unwrap();
        let pick = noted.as_ref().map_or(1, |(_, _, pick)| pick + 1);
        *noted = Some((purpose.to_string(), asking, pick));
    }));
    app.run("mail.new", json!({"to": "kit@example.org", "subject": "Figures"})).unwrap();
    (app, inbox, opened)
}

/// What the picker delivers for the pick it was last opened for, as the browser's host does:
/// read or left unread by the files' sizes ([`reads_of_pick`]), each tagged with the pick.
fn picked(opened: &Opened, files: &[(&str, &[u8])]) -> Vec<Delivered> {
    let (purpose, asking, pick) = opened.lock().unwrap().clone().expect("the picker was opened");
    let sizes: Vec<u64> = files.iter().map(|(_, bytes)| bytes.len() as u64).collect();
    files
        .iter()
        .zip(reads_of_pick(&sizes))
        .map(|((name, bytes), unread)| match unread {
            None => Delivered::picked(name.to_string(), bytes.to_vec(), &purpose, asking, pick),
            Some(why) => Delivered::unread(name.to_string(), why, &purpose, asking, pick),
        })
        .collect()
}

/// The browser: the picker's files arrive together, and each is attached, a message file too.
#[test]
fn every_file_from_the_browser_picker_is_attached() {
    let (mut app, inbox, opened) = browser();
    app.run("ui.attach", json!({})).unwrap();
    assert_eq!(opened.lock().unwrap().as_ref().map(|(p, _, _)| p.as_str()), Some("attach"));
    inbox.lock().unwrap().extend(picked(&opened, &[("minutes.txt", b"one"), ("notes.eml", b"two")]));
    super::drain_inbox(&mut app);
    assert_eq!(attached(&app), ["minutes.txt", "notes.eml"]);
    assert_eq!(said(&app), "Attached 2 files");
    // One of them refused (the message carries as many as it can by then): the rest go in,
    // and the line says how many did and why one didn't.
    for i in 2..199 {
        app.run("compose.attach", json!({"name": format!("f{i}.txt"), "data": "eA=="})).unwrap();
    }
    app.run("ui.attach", json!({})).unwrap();
    inbox.lock().unwrap().extend(picked(&opened, &[("budget.csv", b"three"), ("one-more.txt", b"four")]));
    super::drain_inbox(&mut app);
    assert_eq!(attached(&app).len(), 200);
    assert_eq!(attached(&app)[199], "budget.csv");
    assert!(said(&app).starts_with("Attached 1 file · ") && said(&app).contains("as many attachments"), "{}", said(&app));
}

/// The browser: a file the browser couldn't read (it moved or changed since it was picked) is
/// said, and the others of its pick go in.
#[test]
fn a_file_the_browser_couldnt_read_is_said_and_the_rest_go_in() {
    let (mut app, inbox, opened) = browser();
    app.run("ui.attach", json!({})).unwrap();
    let mut files = picked(&opened, &[("minutes.txt", b"one"), ("budget.csv", b"two")]);
    files.insert(
        1,
        Delivered {
            name: "gone.txt".into(),
            bytes: Err(Unread::Failed("gone.txt couldn't be read".into())),
            asked: files[0].asked.clone(),
            pick: files[0].pick,
        },
    );
    inbox.lock().unwrap().extend(files);
    super::drain_inbox(&mut app);
    assert_eq!(attached(&app), ["minutes.txt", "budget.csv"]);
    assert_eq!(said(&app), "Attached 2 files · gone.txt couldn't be read");
    // One file alone that couldn't be read: said, nothing attached.
    app.run("ui.attach", json!({})).unwrap();
    inbox.lock().unwrap().push(Delivered::unread("lost.txt".into(), Unread::Failed("lost.txt couldn't be read".into()), "attach", None, 9));
    super::drain_inbox(&mut app);
    assert_eq!(attached(&app).len(), 2);
    assert_eq!(said(&app), "lost.txt couldn't be read");
}

/// The browser: more than the most at a time are picked; the host reads the first hundred and
/// delivers the rest unread, which go in and are said as on the desktop.
#[test]
fn more_than_the_most_at_a_time_from_the_browser_picker_are_left_out_and_said() {
    let (mut app, inbox, opened) = browser();
    app.run("ui.attach", json!({})).unwrap();
    let names: Vec<String> = (0..101).map(|i| format!("file-{i:03}.txt")).collect();
    let files: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"x"[..])).collect();
    inbox.lock().unwrap().extend(picked(&opened, &files));
    super::drain_inbox(&mut app);
    assert_eq!(attached(&app).len(), 100);
    assert_eq!(said(&app), "Attached 100 files · 1 more was left out: at most 100 files at a time");
}

/// The browser: each file arrives saying what it was picked for and for which message, so a
/// picker opened for something else meanwhile takes nothing of another's answer, whichever
/// answers first.
#[test]
fn a_picked_file_goes_where_it_was_picked_for_whatever_arrived_meanwhile() {
    let (mut app, inbox, opened) = browser();
    app.run("ui.attach", json!({})).unwrap();
    let to_attach = picked(&opened, &[("minutes.txt", b"one"), ("notes.eml", b"two")]);
    // Before that picker answers, another is opened, for a mailbox.
    app.run("file.open", json!({})).unwrap();
    assert_eq!(opened.lock().unwrap().as_ref().map(|(p, _, _)| p.as_str()), Some("mailbox"));
    let subject = app.session.compose.as_ref().unwrap().subject.clone();
    // The first picker's files are attached, as they were picked for; the second's is a mailbox
    // to open, which this one isn't, and the message being written is untouched either way.
    inbox.lock().unwrap().extend(to_attach);
    super::drain_inbox(&mut app);
    assert_eq!(attached(&app), ["minutes.txt", "notes.eml"]);
    assert_eq!(said(&app), "Attached 2 files");
    inbox.lock().unwrap().extend(picked(&opened, &[("not-a-mailbox.txt", b"three")]));
    super::drain_inbox(&mut app);
    assert_eq!(attached(&app), ["minutes.txt", "notes.eml"]);
    assert_eq!(app.session.compose.as_ref().unwrap().subject, subject);
}

/// What is read of a pick, from the sizes alone: at most the most files, none over the most one
/// attachment may have (never read, and not counted towards the pick), and nothing from the
/// first that would take the pick past the most bytes together.
#[test]
fn which_files_of_a_pick_are_read_is_decided_from_their_sizes_before_any_is_read() {
    let mb = 1u64 << 20;
    assert_eq!(reads_of_pick(&[]), []);
    assert_eq!(reads_of_pick(&[1, 64 * mb, 0]), [None, None, None]);
    // One over the most for an attachment is never read, and doesn't count towards the total.
    let plan = reads_of_pick(&[64 * mb + 1, 64 * mb, 64 * mb, 64 * mb, 64 * mb]);
    assert_eq!(plan, [Some(Unread::TooLarge), None, None, None, None]);
    // From the first that would pass the most together, nothing is read, however small.
    let plan = reads_of_pick(&[60 * mb, 60 * mb, 60 * mb, 60 * mb, 60 * mb, 1]);
    assert_eq!(plan, [None, None, None, None, Some(Unread::PastTotal), Some(Unread::PastTotal)]);
    // Exactly the most together is read; one byte more is not.
    let four = MOST_ATTACHED_BYTES / MOST_ATTACHMENT_BYTES;
    let sizes: Vec<u64> = (0..=four).map(|i| if i == four { 1 } else { MOST_ATTACHMENT_BYTES }).collect();
    let plan = reads_of_pick(&sizes);
    assert!(plan.iter().take(four as usize).all(Option::is_none), "{plan:?}");
    assert_eq!(plan.last(), Some(&Some(Unread::PastTotal)));
    // Past the most files, whatever their sizes, and the pick's bytes are what was read.
    let sizes: Vec<u64> = (0..MOST_ATTACHED + 2).map(|i| if i == MOST_ATTACHED { 300 * mb } else { 1 }).collect();
    let plan = reads_of_pick(&sizes);
    assert!(plan.iter().take(MOST_ATTACHED).all(Option::is_none));
    assert_eq!(&plan[MOST_ATTACHED..], [Some(Unread::PastMost), Some(Unread::PastMost)]);
    // The sum never overflows.
    assert_eq!(reads_of_pick(&[u64::MAX, u64::MAX]), [Some(Unread::TooLarge), Some(Unread::TooLarge)]);
}

/// The desktop: a file larger than a message may carry is refused by its size, before anything
/// of it is read (a sparse file of 65 MB holds nothing), and the others of its pick go in.
#[test]
fn a_file_too_large_to_attach_is_refused_unread_and_the_rest_go_in() {
    let (files, mut paths) = Files::new("too-large", &["minutes.txt", "budget.csv"]);
    let large = files.0.join("video.mp4");
    std::fs::File::create(&large).unwrap().set_len(MOST_ATTACHMENT_BYTES + 1).unwrap();
    paths.insert(1, large.to_string_lossy().to_string());
    let (mut app, _) = writing(Some(paths));
    app.run("ui.attach", json!({})).unwrap();
    assert_eq!(attached(&app), ["minutes.txt", "budget.csv"]);
    assert_eq!(said(&app), "Attached 2 files · video.mp4 is larger than 64 MB");
}

/// The browser: what the host left unread, over the most together or past the most files, is
/// said in the same line as the other refusals.
#[test]
fn files_the_browser_left_unread_past_the_caps_are_said() {
    let (mut app, inbox, opened) = browser();
    app.run("ui.attach", json!({})).unwrap();
    let (purpose, asking, pick) = opened.lock().unwrap().clone().unwrap();
    let unread = |name: &str, why| Delivered::unread(name.into(), why, &purpose, asking, pick);
    inbox.lock().unwrap().extend([
        Delivered::picked("minutes.txt".into(), b"one".to_vec(), &purpose, asking, pick),
        unread("video.mp4", Unread::TooLarge),
        Delivered::picked("budget.csv".into(), b"two".to_vec(), &purpose, asking, pick),
        unread("a.mov", Unread::PastTotal),
        unread("b.mov", Unread::PastTotal),
        unread("c.txt", Unread::PastMost),
    ]);
    super::drain_inbox(&mut app);
    assert_eq!(attached(&app), ["minutes.txt", "budget.csv"]);
    assert_eq!(
        said(&app),
        "Attached 2 files · 1 more was left out: at most 100 files at a time · 2 more were left out: at most 256 MB at a time · video.mp4 is larger than 64 MB"
    );
    // One file alone, too large: said, and nothing read or attached.
    app.run("ui.attach", json!({})).unwrap();
    inbox.lock().unwrap().push(unread("huge.iso", Unread::TooLarge));
    super::drain_inbox(&mut app);
    assert_eq!(attached(&app).len(), 2);
    assert_eq!(said(&app), "huge.iso is larger than 64 MB");
}

/// The browser: two picks for the same message that arrive in one frame are each said by
/// themselves, not run together into one.
#[test]
fn two_picks_for_one_message_arriving_together_are_two_batches() {
    let (mut app, inbox, opened) = browser();
    app.run("ui.attach", json!({})).unwrap();
    let first = picked(&opened, &[("a.txt", b"a"), ("b.txt", b"b")]);
    app.run("ui.attach", json!({})).unwrap();
    let second = picked(&opened, &[("c.txt", b"c"), ("d.txt", b"d"), ("e.txt", b"e")]);
    inbox.lock().unwrap().extend(first.into_iter().chain(second));
    super::drain_inbox(&mut app);
    assert_eq!(attached(&app), ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"]);
    // The last said is the second pick's, not "Attached 5 files".
    assert_eq!(said(&app), "Attached 3 files");
}

/// The desktop: files picked for one message are attached to it even when another message has
/// the keyboard by the time the dialog answers (#286: the one that asked).
#[test]
fn several_files_picked_go_to_the_message_that_asked_whatever_is_current_by_then() {
    let (_files, paths) = Files::new("asked", &["minutes.txt", "budget.csv"]);
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    // A dialog the test answers when it likes, as the owner would.
    let senders: Arc<Mutex<Vec<mpsc::Sender<Option<Vec<String>>>>>> = Arc::default();
    let keep = senders.clone();
    app.services.file_dialog = Some(Box::new(move |_| {
        let (tx, rx) = mpsc::channel();
        keep.lock().unwrap().push(tx);
        rx
    }));
    let asking = app.run("mail.new", json!({})).unwrap()["compose"].as_u64().unwrap();
    assert!(app.run("ui.attach", json!({})).unwrap()["pending"].is_string(), "the dialog is open, unanswered");
    // Meanwhile another message is begun and has the keyboard.
    let other = app.run("mail.new", json!({})).unwrap()["compose"].as_u64().unwrap();
    app.session.show_compose(other);
    assert_eq!(app.session.compose_id(), Some(other));
    senders.lock().unwrap()[0].send(Some(paths)).unwrap();
    let r = super::poll(&mut app).unwrap().unwrap();
    assert_eq!(r["attached"], 2);
    let names = |c: u64| app.session.composing_by_id(c).map(|c| c.attachments.iter().map(|a| a.name.clone()).collect::<Vec<_>>()).unwrap();
    assert_eq!(names(asking), ["minutes.txt", "budget.csv"]);
    assert!(names(other).is_empty());
}

/// The browser: the same, each file tagged with the message that asked.
#[test]
fn several_files_from_the_browser_picker_go_to_the_message_that_asked_whatever_is_current_by_then() {
    let (mut app, inbox, opened) = browser();
    let asking = app.session.compose_id().unwrap();
    app.run("ui.attach", json!({})).unwrap();
    assert_eq!(opened.lock().unwrap().as_ref().map(|(_, a, _)| *a), Some(Some(asking)));
    let other = app.run("mail.new", json!({})).unwrap()["compose"].as_u64().unwrap();
    app.session.show_compose(other);
    assert_eq!(app.session.compose_id(), Some(other));
    inbox.lock().unwrap().extend(picked(&opened, &[("minutes.txt", b"one"), ("budget.csv", b"two")]));
    super::drain_inbox(&mut app);
    let names = |c: u64| app.session.composing_by_id(c).map(|c| c.attachments.iter().map(|a| a.name.clone()).collect::<Vec<_>>()).unwrap();
    assert_eq!(names(asking), ["minutes.txt", "budget.csv"]);
    assert!(names(other).is_empty());
    assert_eq!(said(&app), "Attached 2 files");
}
