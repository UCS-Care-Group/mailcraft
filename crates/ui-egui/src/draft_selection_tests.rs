// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #727: what is selected in the message list is what the reading pane shows. With a message
//! being written in the reading pane (a draft after Continue editing, a New Email), a click on
//! another message, or Up and Down in the list, saves what is written to Drafts and shows the
//! message selected, as the other ways of leaving a message do (#442); a draft with no changes is
//! not saved again; one in a window of its own stays where it is; a draft that can't be saved
//! stays in the reading pane, with the selection as it was, and why is said. Delete then acts on
//! what is shown.

use egui::Key;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a.auto_sync = false;
    let _ = a.execute("ui.dismissReminder", json!({"all": true}));
    a
}

fn folder(a: &MailApp, kind: FolderKind) -> Id {
    let account = a.session.mailbox.default_account().unwrap().id;
    a.session.mailbox.folder_of_kind(account, kind).unwrap()
}

/// The message with `subject` in the folder of `kind`.
fn in_folder(a: &MailApp, kind: FolderKind, subject: &str) -> Option<Id> {
    let f = folder(a, kind);
    a.session.mailbox.messages.values().find(|m| m.folder == f && m.subject == subject).map(|m| m.id)
}

fn draft(a: &MailApp, subject: &str) -> Option<Id> {
    in_folder(a, FolderKind::Drafts, subject)
}

fn body(a: &MailApp, id: Id) -> String {
    a.session.mailbox.message(id).unwrap().body.text.clone()
}

/// The message being written in the reading pane.
fn pane(a: &MailApp) -> Option<u64> {
    crate::windows::compose_window::pane_compose(a)
}

/// Two drafts kept in Drafts, the Drafts folder in view, and nothing being written.
fn two_drafts() -> (MailApp, Id, Id) {
    let mut a = app();
    for subject in ["First draft", "Second draft"] {
        a.execute("mail.new", json!({})).unwrap();
        a.execute("compose.set", json!({"to": "ada@example.org", "subject": subject, "body": "Kept as it was"})).unwrap();
        a.execute("compose.saveDraft", json!({})).unwrap();
        a.execute("compose.close", json!({})).unwrap();
    }
    a.run("folder.select", json!({"kind": "drafts"})).unwrap();
    let (first, second) = (draft(&a, "First draft").unwrap(), draft(&a, "Second draft").unwrap());
    (a, first, second)
}

/// The first draft selected and opened in the reading pane, as Continue editing opens it.
fn continued() -> (MailApp, Id, Id) {
    let (mut a, first, second) = two_drafts();
    a.run("mail.select", json!({"id": first})).unwrap();
    a.run("mail.editDraft", json!({})).unwrap();
    assert!(pane(&a).is_some(), "the draft is in the reading pane");
    (a, first, second)
}

/// What the owner types in the draft being written.
fn typed(a: &mut MailApp) {
    a.execute("compose.set", json!({"body": "Words not saved yet"})).unwrap();
    assert!(a.session.compose.as_ref().unwrap().dirty);
}

#[test]
fn clicking_another_draft_saves_this_one_and_shows_the_one_clicked() {
    let (mut a, _, second) = continued();
    typed(&mut a);
    a.run("mail.select", json!({"id": second})).unwrap();
    assert_eq!(a.session.sel.messages, vec![second]);
    assert!(pane(&a).is_none(), "the reading pane shows the draft clicked");
    assert_eq!(a.session.composing_count(), 0, "the one being written is closed");
    let first = draft(&a, "First draft").expect("still in Drafts");
    assert!(body(&a, first).contains("Words not saved yet"), "saved to Drafts first");
}

#[test]
fn clicking_another_message_saves_and_shows_it_for_a_new_message_too() {
    // A draft after Continue editing, and a New Email, each leave the reading pane for the
    // message clicked, received mail included.
    for new in [false, true] {
        let (mut a, _, _) = continued();
        if new {
            a.execute("compose.close", json!({})).unwrap();
            a.run("mail.new", json!({})).unwrap();
            a.execute("compose.set", json!({"subject": "Brand new", "body": "Something"})).unwrap();
        } else {
            typed(&mut a);
        }
        a.run("folder.select", json!({"kind": "inbox"})).unwrap();
        let inbox = folder(&a, FolderKind::Inbox);
        let other = a.session.mailbox.messages.values().find(|m| m.folder == inbox && !m.draft).unwrap().id;
        a.run("mail.select", json!({"id": other})).unwrap();
        assert_eq!(a.session.sel.messages, vec![other], "new: {new}");
        assert!(pane(&a).is_none(), "new: {new}: the message clicked is shown");
        let kept = if new { draft(&a, "Brand new") } else { draft(&a, "First draft") };
        assert!(kept.is_some(), "new: {new}: saved to Drafts");
    }
}

#[test]
fn moving_in_the_list_from_the_menus_leaves_the_message_too() {
    // Next Message and the rest, from the menu bar or Search commands.
    for (id, params) in [
        ("mail.next", json!({})),
        ("mail.previous", json!({})),
        ("mail.first", json!({})),
        ("mail.last", json!({})),
        ("mail.pageDown", json!({"rows": 5})),
        ("mail.pageUp", json!({"rows": 5})),
        ("mail.selectAll", json!({})),
    ] {
        let (mut a, _, _) = continued();
        typed(&mut a);
        let r = a.run(id, params);
        assert!(r.is_ok(), "{id}: {r:?}");
        assert!(pane(&a).is_none(), "{id}: the reading pane shows what is selected");
        let first = draft(&a, "First draft").unwrap();
        assert!(body(&a, first).contains("Words not saved yet"), "{id}: saved to Drafts first");
    }
}

#[test]
fn a_draft_whose_saved_copy_is_gone_is_saved_again_before_it_goes() {
    let (mut a, first, second) = continued();
    // Deleted for good meanwhile, with nothing typed since.
    a.session.mailbox.messages.remove(&first);
    assert!(!a.session.compose.as_ref().unwrap().dirty);
    a.run("mail.select", json!({"id": second})).unwrap();
    assert!(pane(&a).is_none());
    assert!(draft(&a, "First draft").is_some(), "what was written is in Drafts again");
}

#[test]
fn leaving_saves_every_one_before_closing_any() {
    // Written in, then put aside for a New Email that can't be saved: neither goes.
    let (mut a, first, second) = continued();
    typed(&mut a);
    a.run("mail.new", json!({})).unwrap();
    a.execute("compose.set", json!({"subject": "Nowhere", "body": "More"})).unwrap();
    let d = folder(&a, FolderKind::Drafts);
    a.session.mailbox.folders.iter_mut().find(|f| f.id == d).unwrap().kind = FolderKind::Custom;
    let shown = pane(&a);
    assert!(a.run("mail.select", json!({"id": second})).is_err());
    assert_eq!(a.session.composing_count(), 2, "both still open");
    assert_eq!(pane(&a), shown, "the reading pane shows what it did");
    assert_eq!(a.session.sel.messages, vec![first]);
}

#[test]
fn a_draft_with_no_changes_is_closed_and_not_saved_again() {
    let (mut a, first, second) = continued();
    let before = a.session.mailbox.clone();
    let could_undo = a.session.can_undo();
    a.run("mail.select", json!({"id": second})).unwrap();
    assert!(pane(&a).is_none());
    assert!(a.session.mailbox == before, "nothing was saved");
    assert_eq!(a.session.can_undo(), could_undo, "and nothing to undo");
    assert_eq!(draft(&a, "First draft"), Some(first));
}

#[test]
fn clicking_the_row_of_the_draft_being_written_leaves_it_open() {
    let (mut a, first, _) = continued();
    typed(&mut a);
    a.run("mail.select", json!({"id": first})).unwrap();
    assert!(pane(&a).is_some(), "the draft clicked is the one shown");
    assert!(a.session.compose.as_ref().unwrap().dirty, "and nothing was done to it");
}

#[test]
fn a_message_put_aside_for_another_doesnt_come_back_over_the_one_clicked() {
    let (mut a, _, second) = continued();
    typed(&mut a);
    // New Email puts the draft aside, saved (#286); it would come back to the reading pane once
    // the new one closes.
    a.run("mail.new", json!({})).unwrap();
    a.execute("compose.set", json!({"subject": "Second thoughts", "body": "More"})).unwrap();
    a.run("mail.select", json!({"id": second})).unwrap();
    assert_eq!(a.session.sel.messages, vec![second]);
    assert!(pane(&a).is_none(), "neither is shown over the draft clicked");
    assert!(draft(&a, "First draft").is_some_and(|d| body(&a, d).contains("Words not saved yet")));
    assert!(draft(&a, "Second thoughts").is_some());
}

#[test]
fn a_draft_that_cant_be_saved_stays_with_the_selection_as_it_was() {
    let (mut a, first, second) = continued();
    typed(&mut a);
    let d = folder(&a, FolderKind::Drafts);
    a.session.mailbox.folders.iter_mut().find(|f| f.id == d).unwrap().kind = FolderKind::Custom;
    a.status_msg = None;
    let compose = a.session.compose.clone();
    assert!(a.run("mail.select", json!({"id": second})).is_err());
    assert_eq!(a.session.compose, compose, "still being written");
    assert!(pane(&a).is_some(), "in the reading pane");
    assert_eq!(a.session.sel.messages, vec![first], "the selection is the draft shown");
    assert!(a.status_msg.as_ref().is_some_and(|(said, _)| said.contains("Drafts")), "{:?}", a.status_msg);
}

#[test]
fn a_script_selecting_leaves_the_owners_message_alone() {
    let (mut a, _, second) = continued();
    typed(&mut a);
    a.execute("mail.select", json!({"id": second})).unwrap();
    assert!(pane(&a).is_some(), "only the owner's own click leaves it");
    assert!(a.session.compose.as_ref().unwrap().dirty);
}

// In the window.

fn window(a: MailApp) -> Harness<'static, MailApp> {
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        a,
    );
    h.run_steps(4);
    h
}

fn click_at(h: &mut Harness<'static, MailApp>, pos: egui::Pos2, button: egui::PointerButton) {
    h.event(egui::Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
}

fn row(h: &Harness<'static, MailApp>, id: Id) -> egui::Pos2 {
    h.state().hit.rect("message", id).expect("its row is drawn").center()
}

fn key(h: &mut Harness<'static, MailApp>, key: Key) {
    let m = egui::Modifiers::NONE;
    h.event(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: m });
    h.step();
    h.event(egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: m });
    h.run_steps(3);
}

/// The window with Drafts in view, the first draft selected and Continue editing pressed.
fn continued_in_window() -> (Harness<'static, MailApp>, Id, Id) {
    let (mut a, first, second) = two_drafts();
    a.run("mail.select", json!({"id": first})).unwrap();
    let mut h = window(a);
    h.get_by_label("Continue editing").click();
    h.run_steps(4);
    assert!(pane(h.state()).is_some(), "Continue editing opens it in the reading pane");
    (h, first, second)
}

#[test]
fn a_click_then_delete_deletes_the_message_shown() {
    for ribbon in [false, true] {
        let (mut h, _, second) = continued_in_window();
        typed(h.state_mut());
        let at = row(&h, second);
        click_at(&mut h, at, egui::PointerButton::Primary);
        assert_eq!(h.state().session.sel.messages, vec![second], "ribbon: {ribbon}");
        assert!(pane(h.state()).is_none(), "ribbon: {ribbon}: the draft clicked is shown");
        if ribbon {
            h.state_mut().run("mail.delete", json!({})).unwrap();
            h.run_steps(3);
        } else {
            key(&mut h, Key::Delete);
        }
        assert!(in_folder(h.state(), FolderKind::Deleted, "Second draft").is_some(), "ribbon: {ribbon}: the one shown is deleted");
        let first = draft(h.state(), "First draft").expect("the one written in stays in Drafts");
        assert!(body(h.state(), first).contains("Words not saved yet"), "ribbon: {ribbon}: with what was typed");
    }
}

#[test]
fn the_lists_keys_move_no_hidden_selection_and_after_a_click_show_what_they_select() {
    for k in [Key::ArrowDown, Key::ArrowUp, Key::PageDown, Key::End, Key::Home] {
        let (mut h, first, second) = continued_in_window();
        typed(h.state_mut());
        // The keyboard out of the body (in it, the keys move the caret), as after a click on the
        // ribbon: the key is neither the list's nor takes the message away.
        h.ctx.memory_mut(|m| m.stop_text_input());
        h.run_steps(2);
        key(&mut h, k);
        assert_eq!(h.state().session.sel.messages, vec![first], "{k:?}: the selection stays the draft shown");
        assert!(h.state().session.compose.as_ref().is_some_and(|c| c.dirty), "{k:?}: still being written");
        // A click on a row shows it, and then the keys are the list's again.
        let at = row(&h, second);
        click_at(&mut h, at, egui::PointerButton::Primary);
        assert!(pane(h.state()).is_none(), "{k:?}");
        let rows: Vec<Id> = h.state_mut().session.rows().iter().map(|r| r.id).collect();
        let to = if rows.first() == Some(&second) { Key::ArrowDown } else { Key::ArrowUp };
        key(&mut h, to);
        assert_ne!(h.state().session.sel.messages, vec![second], "{k:?}: {to:?} moves in the list");
    }
}

#[test]
fn ctrl_a_selects_nothing_behind_the_message_being_written() {
    let (mut h, first, _) = continued_in_window();
    typed(h.state_mut());
    h.ctx.memory_mut(|m| m.stop_text_input());
    h.run_steps(2);
    let m = egui::Modifiers::COMMAND;
    h.event(egui::Event::Key { key: Key::A, physical_key: None, pressed: true, repeat: false, modifiers: m });
    h.step();
    h.event(egui::Event::Key { key: Key::A, physical_key: None, pressed: false, repeat: false, modifiers: m });
    h.run_steps(3);
    assert_eq!(h.state().session.sel.messages, vec![first]);
    assert!(pane(h.state()).is_some());
}

#[test]
fn a_right_click_on_another_row_shows_it_too() {
    let (mut h, _, second) = continued_in_window();
    typed(h.state_mut());
    let at = row(&h, second);
    click_at(&mut h, at, egui::PointerButton::Secondary);
    assert_eq!(h.state().session.sel.messages, vec![second]);
    assert!(pane(h.state()).is_none(), "the row its menu acts on is the one shown");
    assert!(draft(h.state(), "First draft").is_some_and(|d| body(h.state(), d).contains("Words not saved yet")));
}

#[test]
fn a_draft_in_a_window_of_its_own_stays_open_through_clicks_in_the_list() {
    let (mut h, _, second) = continued_in_window();
    typed(h.state_mut());
    let compose = h.state().session.compose_id().unwrap();
    h.state_mut().run("compose.popOut", json!({})).unwrap();
    h.run_steps(4);
    assert!(h.state().windows.of_compose(compose).is_some(), "in a window of its own");
    // (Its window lies over the list here, in the main one: the click the row runs.)
    h.state_mut().run("mail.select", json!({"id": second})).unwrap();
    h.run_steps(3);
    let a = h.state();
    assert_eq!(a.session.sel.messages, vec![second]);
    assert!(a.windows.of_compose(compose).is_some(), "still in its window");
    assert!(a.session.composing_by_id(compose).is_some_and(|c| c.dirty), "and nothing was done to it");
}
