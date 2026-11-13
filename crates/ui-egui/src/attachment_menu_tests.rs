// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The `▾` at an attachment chip's right end (#785): it opens the menu a right-click on the chip
//! opens, from the pointer and from the keyboard, in the reading pane and in a message's window.

use std::sync::{Arc, Mutex};

use egui::{Key, PointerButton, Rect, accesskit::Role};
use egui_kittest::kittest::{NodeT, Queryable};
use mailcraft_model::Attachment;
use serde_json::json;

use crate::people_view::testing::{self, Window, click, key};
use crate::reading::attachment_key;

/// Two messages of the sample mailbox that carry files.
const A: u64 = 13;
const B: u64 = 15;

/// The entries of an attachment's menu.
const ENTRIES: [&str; 5] = ["Open", "Save As…", "Save All Attachments…", "Remove Attachment", "Copy"];

type Opened = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

/// The sample mailbox with `A` in the reading pane holding `a_files` and `B` holding one file,
/// and a host that notes what it is asked to open.
fn window_with(a_files: &[&str]) -> (Window, Opened) {
    let mut h = testing::window("view.mail");
    let app = h.state_mut();
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    let file = |name: &str, bytes: &[u8]| Attachment::new(name, "application/pdf", bytes.to_vec());
    app.session.mailbox.message_mut(A).unwrap().attachments = a_files.iter().enumerate().map(|(i, n)| file(n, format!("a{i}").as_bytes())).collect();
    app.session.mailbox.message_mut(B).unwrap().attachments = vec![file("other.pdf", b"b0")];
    let opened: Opened = Default::default();
    let sink = opened.clone();
    app.services.open_file = Some(Box::new(move |name, bytes| {
        sink.lock().unwrap().push((name.to_string(), bytes.to_vec()));
        Ok(())
    }));
    app.run("mail.select", json!({"id": A})).unwrap();
    h.run_steps(5);
    (h, opened)
}

fn window() -> (Window, Opened) {
    window_with(&["notes.pdf"])
}

fn chip(h: &Window, id: u64, index: usize) -> Rect {
    h.state().hit.rect_in(0, "attachment", attachment_key(id, index)).unwrap_or_else(|| panic!("no chip {id}:{index}"))
}

fn arrow(h: &Window, id: u64, index: usize) -> Rect {
    h.state().hit.rect_in(0, "attachmentMenu", attachment_key(id, index)).unwrap_or_else(|| panic!("no ▾ {id}:{index}"))
}

/// The menu entries labelled `entry` (a menu entry's label ends with its shortcut's text, here
/// none, after a space).
fn entry_nodes<'a>(h: &'a Window, entry: &'a str) -> impl Iterator<Item = egui_kittest::Node<'a>> + 'a {
    h.query_all_by_label_contains(entry)
        .filter(move |n| n.accesskit_node().role() == Role::Button && n.accesskit_node().label().is_some_and(|l| l.trim() == entry))
}

/// How many attachment menus are open (each has one Save All Attachments…).
fn menus(h: &Window) -> usize {
    entry_nodes(h, "Save All Attachments…").count()
}

fn menu_has_its_entries(h: &Window) {
    for entry in ENTRIES {
        assert!(entry_nodes(h, entry).next().is_some(), "{entry}");
    }
    assert_eq!(menus(h), 1, "one menu");
}

/// Click the entry `label` of the open menu.
fn choose(h: &mut Window, label: &str) {
    let at = entry_nodes(h, label).next().unwrap_or_else(|| panic!("no {label}")).rect().center();
    click(h, at, PointerButton::Primary, 1);
}

fn opened(o: &Opened) -> Vec<(String, Vec<u8>)> {
    o.lock().unwrap().clone()
}

#[test]
fn a_click_on_the_arrow_opens_the_menu_and_a_second_closes_it() {
    let (mut h, _) = window();
    let at = arrow(&h, A, 0).center();
    click(&mut h, at, PointerButton::Primary, 1);
    assert!(egui::Popup::is_any_open(&h.ctx));
    menu_has_its_entries(&h);
    click(&mut h, at, PointerButton::Primary, 1);
    assert!(!egui::Popup::is_any_open(&h.ctx));
    assert_eq!(menus(&h), 0);
}

/// A right-click on the `▾` never shows two menus, and leaves its left click working. (A context
/// menu on the `▾` shares its popup's id: with one, the left click after a right-click opened
/// nothing.)
#[test]
fn a_right_click_on_the_arrow_shows_no_second_menu() {
    let (mut h, _) = window();
    let at = arrow(&h, A, 0).center();
    click(&mut h, at, PointerButton::Secondary, 1);
    assert!(menus(&h) <= 1, "{} menus", menus(&h));
    // And a left click after it still opens exactly one.
    click(&mut h, at, PointerButton::Primary, 1);
    assert_eq!(menus(&h), 1);
}

#[test]
fn a_right_click_on_the_chip_opens_the_same_menu() {
    let (mut h, _) = window();
    let at = chip(&h, A, 0).center();
    click(&mut h, at, PointerButton::Secondary, 1);
    menu_has_its_entries(&h);
}

#[test]
fn a_double_click_on_the_chip_opens_the_file_and_on_the_arrow_does_not() {
    let (mut h, o) = window_with(&["first.pdf", "second.pdf"]);
    let at = arrow(&h, A, 1).center();
    click(&mut h, at, PointerButton::Primary, 2);
    assert!(opened(&o).is_empty(), "the ▾ opens no file");
    key(&mut h, Key::Escape);
    let at = chip(&h, A, 1).center();
    click(&mut h, at, PointerButton::Primary, 2);
    assert_eq!(opened(&o), vec![("second.pdf".to_string(), b"a1".to_vec())]);
}

#[test]
fn open_from_the_arrow_opens_that_file() {
    let (mut h, o) = window_with(&["first.pdf", "second.pdf"]);
    let at = arrow(&h, A, 1).center();
    click(&mut h, at, PointerButton::Primary, 1);
    choose(&mut h, "Open");
    assert_eq!(opened(&o), vec![("second.pdf".to_string(), b"a1".to_vec())]);
    assert!(!egui::Popup::is_any_open(&h.ctx), "the menu closed");
}

/// Opening the second chip's menu closes the first's.
#[test]
fn one_menu_at_a_time() {
    let (mut h, o) = window_with(&["first.pdf", "second.pdf"]);
    let first = arrow(&h, A, 0).center();
    click(&mut h, first, PointerButton::Primary, 1);
    assert_eq!(menus(&h), 1);
    let second = arrow(&h, A, 1).center();
    click(&mut h, second, PointerButton::Primary, 1);
    assert_eq!(menus(&h), 1, "the second ▾'s menu, in place of the first's");
    choose(&mut h, "Open");
    assert_eq!(opened(&o), vec![("second.pdf".to_string(), b"a1".to_vec())]);
}

#[test]
fn the_keyboard_reaches_the_arrow_and_opens_the_menu() {
    let (mut h, _) = window_with(&["notes.pdf"]);
    h.get_by_role_and_label(Role::Button, "notes.pdf").focus();
    h.run_steps(2);
    assert!(h.get_by_role_and_label(Role::Button, "notes.pdf").is_focused(), "the chip takes the keyboard");
    key(&mut h, Key::Tab);
    let label = "Attachment options for notes.pdf";
    assert!(h.get_by_role_and_label(Role::Button, label).is_focused(), "Tab goes on to the ▾");
    for open in [Key::Enter, Key::Space] {
        key(&mut h, open);
        assert!(egui::Popup::is_any_open(&h.ctx), "{open:?} opens the menu");
        menu_has_its_entries(&h);
        key(&mut h, Key::Escape);
        assert!(!egui::Popup::is_any_open(&h.ctx), "Escape closes it after {open:?}");
        h.get_by_role_and_label(Role::Button, label).focus();
        h.run_steps(2);
    }
}

/// With the menu open from the keyboard, the arrows move through its entries and Enter takes
/// the one that has the keyboard.
#[test]
fn the_keyboard_moves_into_the_menu_and_enter_opens_the_file() {
    let (mut h, o) = window_with(&["first.pdf", "second.pdf"]);
    let label = "Attachment options for second.pdf";
    h.get_by_role_and_label(Role::Button, label).focus();
    h.run_steps(2);
    key(&mut h, Key::Enter);
    assert!(egui::Popup::is_any_open(&h.ctx), "Enter opens the menu");
    let focused = |h: &Window, entry: &str| entry_nodes(h, entry).next().unwrap_or_else(|| panic!("no {entry}")).is_focused();
    key(&mut h, Key::ArrowDown);
    assert!(focused(&h, "Open"), "Down goes into the menu, to its first entry");
    key(&mut h, Key::ArrowDown);
    assert!(focused(&h, "Save As…"), "Down again goes on to the next");
    key(&mut h, Key::ArrowUp);
    assert!(focused(&h, "Open"), "Up comes back");
    assert!(opened(&o).is_empty(), "moving opens nothing");
    key(&mut h, Key::Enter);
    assert_eq!(opened(&o), vec![("second.pdf".to_string(), b"a1".to_vec())]);
    assert!(!egui::Popup::is_any_open(&h.ctx), "the menu closed");
}

/// Enter on the chip's own part does what it did before the `▾`: nothing.
#[test]
fn enter_on_the_chip_opens_nothing() {
    let (mut h, o) = window();
    h.get_by_role_and_label(Role::Button, "notes.pdf").focus();
    h.run_steps(2);
    key(&mut h, Key::Enter);
    assert!(opened(&o).is_empty());
    assert!(!egui::Popup::is_any_open(&h.ctx));
}

/// The same index in the reading pane and in another message's window: each `▾` is its own and
/// opens its own message's file; the same message in the pane and a window opens one menu.
#[test]
fn the_pane_and_a_window_each_have_their_own_arrow() {
    let (mut h, o) = window();
    let w = h.state_mut().run("ui.openWindow", json!({"message": B})).unwrap()["window"].as_u64().unwrap();
    h.run_steps(8);
    let in_window = h.state().hit.rect_in(w, "attachmentMenu", attachment_key(B, 0)).expect("the window's ▾");
    click(&mut h, in_window.center(), PointerButton::Primary, 1);
    assert_eq!(menus(&h), 1);
    choose(&mut h, "Open");
    assert_eq!(opened(&o), vec![("other.pdf".to_string(), b"b0".to_vec())]);

    // The message in the pane in a window too.
    let w2 = h.state_mut().run("ui.openWindow", json!({"message": A})).unwrap()["window"].as_u64().unwrap();
    h.run_steps(8);
    let in_window = h.state().hit.rect_in(w2, "attachmentMenu", attachment_key(A, 0)).expect("the second window's ▾");
    click(&mut h, in_window.center(), PointerButton::Primary, 1);
    assert_eq!(menus(&h), 1, "only the window's menu");
    choose(&mut h, "Open");
    assert_eq!(opened(&o).last(), Some(&("notes.pdf".to_string(), b"a0".to_vec())));
}

/// Remove Attachment is greyed in the `▾`'s menu exactly when it is in the right-click menu.
#[test]
fn a_greyed_entry_is_greyed_in_both_menus() {
    let (mut h, _) = window();
    let w = h.state_mut().run("ui.openWindow", json!({"message": B})).unwrap()["window"].as_u64().unwrap();
    // Nothing selected in the main window: the commands that need a selected message are greyed.
    h.state_mut().session.sel.messages.clear();
    h.run_steps(8);
    let remove = |h: &Window| entry_nodes(h, "Remove Attachment").next().expect("Remove Attachment").accesskit_node().is_disabled();
    let drop = h.state().hit.rect_in(w, "attachmentMenu", attachment_key(B, 0)).unwrap();
    let body = h.state().hit.rect_in(w, "attachment", attachment_key(B, 0)).unwrap();
    click(&mut h, drop.center(), PointerButton::Primary, 1);
    let from_arrow = remove(&h);
    key(&mut h, Key::Escape);
    click(&mut h, body.center(), PointerButton::Secondary, 1);
    let from_right_click = remove(&h);
    assert!(from_arrow, "greyed from the ▾");
    assert_eq!(from_arrow, from_right_click);
}

/// A message file that is not filed: its `▾` is greyed, takes no keyboard and opens nothing.
#[test]
fn an_unfiled_message_has_a_greyed_arrow() {
    let dir = std::env::temp_dir().join(format!("mailcraft-attachment-menu-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("handed.eml");
    let eml = "From: a@example.org\r\nTo: b@example.org\r\nSubject: Handed over\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nSee the file.\r\n--b\r\nContent-Type: application/pdf; name=\"handed.pdf\"\r\nContent-Disposition: attachment; filename=\"handed.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n--b--\r\n";
    std::fs::write(&path, eml).unwrap();
    let (mut h, o) = window();
    h.state_mut().open_from_system(&path.to_string_lossy());
    h.run_steps(8);
    let w = crate::windows::inspect(h.state()).as_array().unwrap().iter().find(|w| w["kind"] == "file").and_then(|w| w["id"].as_u64());
    let w = w.expect("a window for the file");
    let drop = h.state().hit.rect_in(w, "attachmentMenu", attachment_key(0, 0)).expect("the file's ▾");
    click(&mut h, drop.center(), PointerButton::Primary, 1);
    assert!(!egui::Popup::is_any_open(&h.ctx), "no menu");
    assert!(h.query_by_label("Attachment options for handed.pdf").is_none(), "not a button");
    // Its hover text says why.
    // (A click hides tooltips until the pointer moves.)
    h.hover_at(drop.center() + egui::vec2(0.0, 200.0));
    h.run_steps(5);
    h.hover_at(drop.center());
    h.run_steps(60);
    assert!(h.query_all_by_label_contains("Move the message to a folder to open or save its attachments").next().is_some());
    assert!(opened(&o).is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

/// A very long name and a right-to-left override in a narrow window: nothing panics, the chip
/// keeps its width bound, and the `▾` stays inside it.
#[test]
fn long_and_reversed_names_keep_the_chip_bounded() {
    let long = format!("{}.pdf", "x".repeat(10_000));
    let (mut h, _) = window_with(&[&long, "\u{202E}fdp.exe"]);
    // Narrow, and tall enough for the chips, which wrap under each other, to be on screen.
    h.set_size(egui::vec2(700.0, 1600.0));
    h.run_steps(5);
    for index in 0..2 {
        let (body, drop) = (chip(&h, A, index), arrow(&h, A, index));
        assert!(body.width() + drop.width() <= 220.0 + 44.0 + crate::ribbon::DROP_W + 0.5, "{index}: {}", body.width());
        assert!((drop.min.x - body.max.x).abs() < 0.5, "{index}: the ▾ is at the chip's right end");
    }
    let at = arrow(&h, A, 1).center();
    click(&mut h, at, PointerButton::Primary, 1);
    assert_eq!(menus(&h), 1);
}

/// What the host was asked to write for a Copy, and the files put on the clipboard (#807).
#[derive(Clone, Default)]
struct Copied {
    written: std::rc::Rc<std::cell::RefCell<Vec<(String, Vec<u8>)>>>,
    put: std::rc::Rc<std::cell::RefCell<Vec<Vec<std::path::PathBuf>>>>,
    /// Where the host writes the files, as a folder of its own: they are real files, for the
    /// window to find the one it wrote before.
    dir: std::rc::Rc<Dir>,
}

/// A folder of its own for a test, removed with it.
struct Dir(std::path::PathBuf);

impl Default for Dir {
    fn default() -> Dir {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mailcraft-copied-menu-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl crate::RichClipboard for Copied {
    fn put(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn html(&self) -> Option<String> {
        None
    }
    fn put_files(&self, paths: &[std::path::PathBuf]) -> Result<(), String> {
        self.put.borrow_mut().push(paths.to_vec());
        Ok(())
    }
}

/// [`window_with`], with a host that writes copies (to nowhere) and a clipboard that takes files.
fn copying(a_files: &[&str]) -> (Window, Copied) {
    let (mut h, _) = window_with(a_files);
    let copied = Copied::default();
    let app = h.state_mut();
    app.services.clipboard = Some(Box::new(copied.clone()));
    let sink = copied.written.clone();
    let dir = copied.dir.clone();
    app.services.copy_file = Some(Box::new(move |name, bytes| {
        let mut written = sink.borrow_mut();
        written.push((name.to_string(), bytes.to_vec()));
        let path = dir.0.join(written.len().to_string()).join(mailcraft_engine::opening::save_name(name));
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        Ok(path)
    }));
    (h, copied)
}

/// The file the host wrote the `n`th time (from 1), as the clipboard was told of it.
fn written(c: &Copied, n: usize, name: &str) -> std::path::PathBuf {
    c.dir.0.join(n.to_string()).join(mailcraft_engine::opening::save_name(name))
}

fn put(c: &Copied) -> Vec<Vec<std::path::PathBuf>> {
    c.put.borrow().clone()
}

#[test]
fn copy_from_the_menu_puts_that_file_on_the_clipboard() {
    let (mut h, c) = copying(&["first.pdf", "second.pdf"]);
    let at = arrow(&h, A, 1).center();
    click(&mut h, at, PointerButton::Primary, 1);
    choose(&mut h, "Copy");
    assert_eq!(*c.written.borrow(), vec![("second.pdf".to_string(), b"a1".to_vec())]);
    assert_eq!(put(&c), vec![vec![written(&c, 1, "second.pdf")]]);
    assert!(!egui::Popup::is_any_open(&h.ctx), "the menu closed");
    // And from a message's own window, its own file.
    let w = h.state_mut().run("ui.openWindow", json!({"message": B})).unwrap()["window"].as_u64().unwrap();
    h.run_steps(8);
    let in_window = h.state().hit.rect_in(w, "attachmentMenu", attachment_key(B, 0)).expect("the window's ▾");
    click(&mut h, in_window.center(), PointerButton::Primary, 1);
    choose(&mut h, "Copy");
    assert_eq!(c.written.borrow().last(), Some(&("other.pdf".to_string(), b"b0".to_vec())));
}

/// Whether the last frame handed text to the clipboard.
fn text_copied(h: &Window) -> bool {
    h.output().platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(_)))
}

/// Ctrl+C on an attachment picked with a click copies the file, and not the message's text that
/// is selected: the text's copy would reach the clipboard after the file and take its place.
#[test]
fn ctrl_c_on_a_picked_attachment_copies_the_file_and_not_the_text() {
    let (mut h, c) = copying(&["notes.pdf"]);
    // Text of the message selected: below the chip, in the body, then Ctrl+A.
    let body = chip(&h, A, 0).center() + egui::vec2(0.0, 90.0);
    click(&mut h, body, PointerButton::Primary, 1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    h.run_steps(2);
    h.event(egui::Event::Copy);
    h.step();
    assert!(text_copied(&h), "the selected text is copied while no attachment is picked");
    assert!(put(&c).is_empty());
    // A click picks the attachment, without taking the keyboard from the list.
    let at = chip(&h, A, 0).center();
    click(&mut h, at, PointerButton::Primary, 1);
    assert!(!h.get_by_role_and_label(Role::Button, "notes.pdf").is_focused());
    h.event(egui::Event::Copy);
    h.step();
    assert_eq!(put(&c), vec![vec![written(&c, 1, "notes.pdf")]]);
    assert!(!text_copied(&h), "no text went on the clipboard over the file");
    h.run_steps(2);
    // A press anywhere else lets go of it: Ctrl+C is the text's again.
    click(&mut h, body, PointerButton::Primary, 1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    h.run_steps(2);
    h.event(egui::Event::Copy);
    h.step();
    assert!(text_copied(&h));
    assert_eq!(put(&c).len(), 1);
    h.run_steps(2);
    // From the keyboard: Tab to the chip, then Ctrl+C.
    h.get_by_role_and_label(Role::Button, "notes.pdf").focus();
    h.run_steps(2);
    h.event(egui::Event::Copy);
    h.run_steps(2);
    assert_eq!(put(&c).len(), 2);
}

/// The keyboard moving on from a picked attachment to another chip lets go of the pick: Ctrl+C
/// copies the chip that has the keyboard, not the one clicked before.
#[test]
fn ctrl_c_copies_the_focused_chip_over_a_picked_one() {
    let (mut h, c) = copying(&["first.pdf", "second.pdf"]);
    let at = chip(&h, A, 0).center();
    click(&mut h, at, PointerButton::Primary, 1);
    // Tab on to the second chip.
    h.get_by_role_and_label(Role::Button, "second.pdf").focus();
    h.run_steps(2);
    h.event(egui::Event::Copy);
    h.run_steps(2);
    assert_eq!(put(&c), vec![vec![written(&c, 1, "second.pdf")]]);
    assert_eq!(*c.written.borrow(), vec![("second.pdf".to_string(), b"a1".to_vec())]);
}

/// After a click on an attachment, the list's keys are still the list's (Down goes on to the
/// next message), and Escape lets go of the attachment.
#[test]
fn picking_an_attachment_leaves_the_keys_to_the_list() {
    let (mut h, c) = copying(&["notes.pdf"]);
    let at = chip(&h, A, 0).center();
    click(&mut h, at, PointerButton::Primary, 1);
    key(&mut h, Key::Escape);
    h.event(egui::Event::Copy);
    h.run_steps(2);
    assert!(put(&c).is_empty(), "Escape let go of it");
    click(&mut h, at, PointerButton::Primary, 1);
    key(&mut h, Key::ArrowDown);
    assert_ne!(h.state().session.sel.focus(), Some(A), "Down moved on in the list");
}

/// The control channel's click on Copy is no Copy of the owner's: nothing goes on the clipboard.
#[test]
fn the_control_channel_copies_nothing() {
    let (mut h, c) = copying(&["notes.pdf"]);
    h.state_mut().driven = true;
    let e = h.state_mut().run("mail.copyAttachment", json!({"id": A, "index": 0})).unwrap_err();
    assert!(e.contains("Only you"), "{e}");
    assert!(put(&c).is_empty() && c.written.borrow().is_empty());
}

/// Holding Ctrl+C comes as a Copy for every key repeat: the file is written once, and put on the
/// clipboard each time; one the host has tidied away since is written again.
#[test]
fn a_held_ctrl_c_writes_the_attachment_once() {
    let (mut h, c) = copying(&["notes.pdf", "other.pdf"]);
    let at = chip(&h, A, 0).center();
    click(&mut h, at, PointerButton::Primary, 1);
    for _ in 0..30 {
        h.event(egui::Event::Copy);
        h.step();
    }
    assert_eq!(c.written.borrow().len(), 1, "written once");
    assert_eq!(put(&c), vec![vec![written(&c, 1, "notes.pdf")]; 30]);
    // Another attachment is a file of its own.
    let other = chip(&h, A, 1).center();
    click(&mut h, other, PointerButton::Primary, 1);
    h.event(egui::Event::Copy);
    h.step();
    assert_eq!(c.written.borrow().len(), 2);
    // The file gone (tidied, or capped): written anew.
    std::fs::remove_dir_all(&c.dir.0).unwrap();
    h.event(egui::Event::Copy);
    h.step();
    assert_eq!(c.written.borrow().len(), 3);
    assert!(written(&c, 3, "other.pdf").is_file());
}

/// The macOS menu bar's Edit › Copy (⌘C) is the owner's own Copy, as the key is: it copies the
/// picked attachment, where the control channel's input is refused.
#[test]
fn the_menu_bars_copy_copies_a_picked_attachment_but_the_control_channels_does_not() {
    use crate::menubar::{Action, EditKey, run};
    let (mut h, c) = copying(&["notes.pdf"]);
    let at = chip(&h, A, 0).center();
    click(&mut h, at, PointerButton::Primary, 1);
    run(h.state_mut(), &Action::Edit(EditKey::Copy), String::new);
    assert!(h.state().synthetic.is_empty(), "not the control channel's queue");
    let mut raw = std::mem::take(h.input_mut());
    h.state_mut().raw_input_hook(&mut raw);
    assert!(!h.state().driven, "the owner's input");
    *h.input_mut() = raw;
    h.step();
    assert_eq!(put(&c), vec![vec![written(&c, 1, "notes.pdf")]]);
    // The control channel's own Copy, driven through the same queue it uses, is still refused.
    h.state_mut().synthetic.push(egui::Event::Copy);
    let mut raw = std::mem::take(h.input_mut());
    h.state_mut().raw_input_hook(&mut raw);
    assert!(h.state().driven);
    *h.input_mut() = raw;
    h.step();
    assert_eq!(put(&c).len(), 1, "nothing more on the clipboard");
    let said = h.state().status_msg.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
    assert!(said.contains("Only you can copy"), "{said}");
}

/// Programs are copied as Save As saves them (nothing is opened), and where there is no host to
/// write the copy (the browser) Copy says so and puts nothing on the clipboard.
#[test]
fn any_kind_is_copied_and_without_a_host_copy_says_why() {
    let (mut h, c) = copying(&["setup.exe", "README"]);
    let a = h.state_mut().run("mail.copyAttachment", json!({"id": A, "index": 0})).unwrap();
    let b = h.state_mut().run("mail.copyAttachment", json!({"id": A, "index": 1})).unwrap();
    // The host is given the attachment's own name and names the file as Save As does.
    let names: Vec<String> = c.written.borrow().iter().map(|(n, _)| n.clone()).collect();
    assert_eq!(names, ["setup.exe", "README"]);
    assert_eq!((a["name"].as_str(), b["name"].as_str()), (Some("setup.exe"), Some("README.bin")));
    h.state_mut().services.copy_file = None;
    let e = h.state_mut().run("mail.copyAttachment", json!({"id": A, "index": 0})).unwrap_err();
    assert!(e.contains("can't be copied here") && e.contains("Save As"), "{e}");
    assert_eq!(put(&c).len(), 2);
}

#[test]
fn one_click_previews_the_attachment_and_can_turn_a_page() {
    use crate::attachment_preview::{Format, Host, Page};
    use std::sync::mpsc;

    let (mut h, opened_files) = window();
    let received = Arc::new(Mutex::new(Vec::new()));
    let recorded = received.clone();
    let bytes = b"%PDF-1.7 test input".to_vec();
    let app = h.state_mut();
    app.ui.attachment_previews = true;
    app.session.mailbox.message_mut(A).unwrap().attachments = vec![Attachment::new("notes.pdf", "application/pdf", bytes.clone())];
    app.services.attachment_preview = Some(Host {
        pdf: true,
        images: false,
        render: Box::new(move |request| {
            assert_eq!(request.format, Format::Pdf);
            recorded.lock().unwrap().push((request.page, request.load().unwrap().to_vec()));
            let (send, receive) = mpsc::channel();
            send.send(Ok(Page { image: egui::ColorImage::filled([2, 2], egui::Color32::WHITE), pages: 2 })).unwrap();
            receive
        }),
    });
    h.run_steps(4);
    let at = chip(&h, A, 0).center();
    click(&mut h, at, PointerButton::Primary, 1);
    h.run_steps(30);
    let next = h.state().hit.rect("attachmentPreviewNext", A).unwrap().center();
    click(&mut h, next, PointerButton::Primary, 1);
    assert_eq!(*received.lock().unwrap(), vec![(1, bytes.clone()), (2, bytes)]);
    assert!(opened(&opened_files).is_empty());
    let back = h.state().hit.rect("attachmentPreviewBack", A).unwrap().center();
    click(&mut h, back, PointerButton::Primary, 1);
    h.run_steps(2);
    assert!(h.state().hit.rect("attachmentPreviewBack", A).is_none());
}

#[test]
fn without_a_helper_single_click_keeps_the_message_and_open_still_works() {
    let (mut h, opened_files) = window();
    let at = chip(&h, A, 0).center();
    click(&mut h, at, PointerButton::Primary, 1);
    h.run_steps(10);
    assert!(h.state().hit.rect("attachmentPreviewBack", A).is_none());
    assert!(opened(&opened_files).is_empty());
    let menu = arrow(&h, A, 0).center();
    click(&mut h, menu, PointerButton::Primary, 1);
    assert!(entry_nodes(&h, "Preview Attachment").next().is_none());
    choose(&mut h, "Open");
    assert_eq!(opened(&opened_files), vec![("notes.pdf".into(), b"a0".to_vec())]);
}

#[test]
fn a_disguised_pdf_and_an_oversized_attachment_never_start_the_helper() {
    use crate::attachment_preview::{Host, MAX_INPUT_BYTES};
    let (mut h, _) = window();
    let app = h.state_mut();
    app.ui.attachment_previews = true;
    app.services.attachment_preview =
        Some(Host { pdf: true, images: false, render: Box::new(|_| panic!("an invalid attachment reached the helper")) });
    let file = app.session.mailbox.message_mut(A).unwrap().attachments.first_mut().unwrap();
    file.data = Arc::new(b"<html>not a PDF</html>".to_vec());
    assert!(app.execute("ui.previewAttachment", json!({"id":A, "index":0})).is_err());
    let file = app.session.mailbox.message_mut(A).unwrap().attachments.first_mut().unwrap();
    file.data = Arc::new(Vec::new());
    file.kept = Some(mailcraft_model::ContentRef { sha256: [0; 32], len: MAX_INPUT_BYTES as u64 + 1 });
    assert!(!crate::attachment_preview::available(app, A, 0));
    assert!(app.execute("ui.previewAttachment", json!({"id":A, "index":0})).is_err());
}

#[test]
fn previews_are_off_by_default_and_double_click_only_opens_the_file() {
    use crate::attachment_preview::Host;
    let (mut h, opened_files) = window();
    let bytes = b"%PDF-1.7 test input".to_vec();
    let app = h.state_mut();
    app.session.mailbox.message_mut(A).unwrap().attachments = vec![Attachment::new("notes.pdf", "application/pdf", bytes.clone())];
    app.services.attachment_preview = Some(Host { pdf: true, images: false, render: Box::new(|_| panic!("no preview expected")) });
    h.run_steps(4);
    let at = chip(&h, A, 0).center();
    click(&mut h, at, PointerButton::Primary, 1);
    h.run_steps(30);
    assert!(opened(&opened_files).is_empty());
    assert!(!crate::attachment_preview::available(h.state(), A, 0));
    h.state_mut().ui.attachment_previews = true;
    click(&mut h, at, PointerButton::Primary, 2);
    h.run_steps(30);
    assert_eq!(opened(&opened_files), vec![("notes.pdf".to_string(), bytes)]);
    assert!(h.state().hit.rect("attachmentPreviewBack", A).is_none());
}

/// In Junk itself, only the attachment menu can start a preview.
#[test]
fn junk_requires_the_explicit_attachment_menu() {
    assert_junk_preview_restrictions(false);
}

/// A folder inside Junk inherits the same restrictions.
#[test]
fn a_subfolder_of_junk_requires_the_explicit_attachment_menu() {
    assert_junk_preview_restrictions(true);
}

/// Check both preview entry points and the explicit menu in the requested Junk folder.
fn assert_junk_preview_restrictions(in_subfolder: bool) {
    use crate::attachment_preview::{Host, Page};
    use std::sync::mpsc;

    let (mut h, _) = window();
    let app = h.state_mut();
    app.ui.attachment_previews = true;
    let message = app.session.mailbox.message(A).unwrap();
    let account = message.account;
    let junk = app.session.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Junk).unwrap();
    let folder = if in_subfolder { app.session.mailbox.add_folder(account, Some(junk), "Held").unwrap() } else { junk };
    let message = app.session.mailbox.message_mut(A).unwrap();
    message.attachments = vec![Attachment::new("notes.pdf", "application/pdf", b"%PDF-1.7".to_vec())];
    app.execute("mail.move", json!({"id": A, "folder": folder})).unwrap();
    app.execute("folder.select", json!({"id": folder})).unwrap();
    app.execute("mail.select", json!({"id": A})).unwrap();
    app.services.attachment_preview = Some(Host {
        pdf: true,
        images: false,
        render: Box::new(|request| {
            let (send, receive) = mpsc::channel();
            send.send(request.load().map(|_| Page { image: egui::ColorImage::filled([2, 2], egui::Color32::WHITE), pages: 1 })).unwrap();
            receive
        }),
    });
    let refused = app.execute("ui.previewAttachment", json!({"id": A, "index": 0})).unwrap_err();
    assert_eq!(refused, "In Junk E-mail, preview an attachment from its menu.");
    h.run_steps(4);
    let at = chip(&h, A, 0).center();
    click(&mut h, at, PointerButton::Primary, 1);
    h.run_steps(30);
    assert!(h.state().hit.rect("attachmentPreviewBack", A).is_none());
    let at = arrow(&h, A, 0).center();
    click(&mut h, at, PointerButton::Primary, 1);
    choose(&mut h, "Preview Attachment");
    h.run_steps(5);
    assert!(h.state().hit.rect("attachmentPreviewImage", A).is_some());
}

#[test]
fn the_latest_preview_retries_busy_and_is_discarded_when_its_attachment_is_removed() {
    use crate::attachment_preview::{Error, Host, Page, open};
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;

    let (mut h, _) = window_with(&["one.pdf", "two.pdf"]);
    let app = h.state_mut();
    app.ui.attachment_previews = true;
    for file in &mut app.session.mailbox.message_mut(A).unwrap().attachments {
        file.data = Arc::new(format!("%PDF-1.7 {}", file.name).into_bytes());
    }
    let first = std::sync::atomic::AtomicBool::new(true);
    let old = Arc::new(Mutex::new(None::<Arc<std::sync::atomic::AtomicBool>>));
    let cancelled = old.clone();
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let recorded = attempts.clone();
    app.services.attachment_preview = Some(Host {
        pdf: true,
        images: false,
        render: Box::new(move |request| {
            let bytes = request.load().unwrap();
            let (send, receive) = mpsc::channel();
            recorded.lock().unwrap().push(bytes.clone());
            if bytes.ends_with(b"one.pdf") {
                *cancelled.lock().unwrap() = Some(request.cancel);
                return receive;
            }
            if first.swap(false, Ordering::Relaxed) {
                send.send(Err(Error::Busy)).unwrap();
            } else {
                send.send(Ok(Page { image: egui::ColorImage::filled([2, 2], egui::Color32::WHITE), pages: 1 })).unwrap();
            }
            receive
        }),
    });
    open(app, egui::ViewportId::ROOT, A, 0).unwrap();
    open(app, egui::ViewportId::ROOT, A, 1).unwrap();
    assert!(old.lock().unwrap().as_ref().unwrap().load(Ordering::Relaxed));
    h.run_steps(30);
    assert_eq!(attempts.lock().unwrap().len(), 3);
    assert!(h.state().hit.rect("attachmentPreviewImage", A).is_some());
    h.state_mut().session.mailbox.message_mut(A).unwrap().attachments.remove(0);
    h.run_steps(4);
    assert!(h.state().hit.rect("attachmentPreviewBack", A).is_none());
}
