// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::sync::{Arc, Mutex};

use egui::{Key, PointerButton};
use egui_kittest::kittest::{NodeT, Queryable};
use mailcraft_engine::store::{self, Digest};
use mailcraft_model::Attachment;
use serde_json::json;

use super::tests::app;
use crate::MailApp;
use crate::attachment_open::OPEN_DRAFT;
use crate::people_view::testing::{click, window};

type Opened = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

fn record_open(app: &mut MailApp) -> Opened {
    let opened: Opened = Default::default();
    let sink = opened.clone();
    app.services.open_file = Some(Box::new(move |name, bytes| {
        sink.lock().unwrap().push((name.to_owned(), bytes.to_vec()));
        Ok(())
    }));
    opened
}

fn draft(app: &mut MailApp, subject: &str, data: &str) -> u64 {
    let compose = app.run("mail.new", json!({"to": "maya@lumen.example", "subject": subject, "body": "Please check this."})).unwrap()["compose"]
        .as_u64()
        .unwrap();
    app.execute("compose.attach", json!({"compose": compose, "name": "report.txt", "data": data})).unwrap();
    compose
}

#[test]
fn the_draft_chip_opens_by_double_click_menu_and_keyboard_and_removes_separately() {
    let mut h = window("view.mail");
    let app = h.state_mut();
    let opened = record_open(app);
    let compose = draft(app, "Quarterly report", "ZHJhZnQgY29weQ==");
    let before = app.session.composing_by_id(compose).unwrap().attachments.clone();
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    h.run_steps(5);
    let chip = h.state().hit.rect_in(0, "draftAttachment", 0).unwrap().center();
    click(&mut h, chip, PointerButton::Primary, 1);
    assert!(opened.lock().unwrap().is_empty(), "a single pointer click should not launch another app");
    click(&mut h, chip, PointerButton::Primary, 2);
    assert_eq!(opened.lock().unwrap().as_slice(), &[("report.txt".into(), b"draft copy".to_vec())]);

    click(&mut h, chip, PointerButton::Secondary, 1);
    let open = h.query_all_by_label_contains("Open").find(|n| n.accesskit_node().label().is_some_and(|label| label.trim() == "Open")).unwrap();
    let at = open.rect().center();
    click(&mut h, at, PointerButton::Primary, 1);
    assert_eq!(opened.lock().unwrap().len(), 2, "Open from the menu hands the file to the host");

    click(&mut h, chip, PointerButton::Primary, 1);
    super::tests::key(&mut h, Key::Enter);
    assert_eq!(opened.lock().unwrap().len(), 3, "Enter opens the selected attachment");
    let current = h.state().session.composing_by_id(compose).unwrap();
    assert_eq!(current.subject, "Quarterly report");
    assert!(current.body.plain().contains("Please check this."));
    assert_eq!(current.attachments, before);
    assert!(current.draft.is_none(), "opening does not save or send the draft");

    let remove = h.state().hit.rect_in(0, "draftAttachmentRemove", 0).unwrap().center();
    click(&mut h, remove, PointerButton::Primary, 1);
    assert!(h.state().session.composing_by_id(compose).unwrap().attachments.is_empty());
    assert_eq!(opened.lock().unwrap().len(), 3);
}

#[test]
fn a_pop_out_opens_its_own_copy_even_with_a_same_named_file_in_the_other_draft() {
    let mut h = window("view.mail");
    let app = h.state_mut();
    let opened = record_open(app);
    let compose = draft(app, "Pop-out draft", "cG9wLW91dA==");
    let pop = app.run("ui.openWindow", json!({"compose": compose})).unwrap()["window"].as_u64().unwrap();
    let other = draft(app, "Reading pane draft", "cmVhZGluZyBwYW5l");
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    h.run_steps(5);
    let chip = h.state().hit.rect_in(pop, "draftAttachment", 0).unwrap().center();
    click(&mut h, chip, PointerButton::Primary, 2);
    assert_eq!(opened.lock().unwrap().as_slice(), &[("report.txt".into(), b"pop-out".to_vec())]);
    assert_eq!(h.state().session.composing_by_id(other).unwrap().attachments[0].data.as_slice(), b"reading pane");
}

#[test]
fn a_reopened_draft_opens_the_bytes_of_a_stored_attachment() {
    let mut app = app();
    let opened = record_open(&mut app);
    draft(&mut app, "Saved report", "b3JpZ2luYWw=");
    let saved = app.execute("compose.saveDraft", json!({})).unwrap()["draft"].as_u64().unwrap();
    app.execute("compose.close", json!({})).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("draft.mailcraft");
    let bytes = b"original";
    let reference = store::stash(&path, &Digest::of(bytes), bytes).unwrap();
    app.session.path = Some(path);
    let file = &mut app.session.mailbox.message_mut(saved).unwrap().attachments[0];
    file.data = Arc::default();
    file.kept = Some(reference);
    let compose = app.execute("mail.editDraft", json!({"id": saved})).unwrap()["compose"].as_u64().unwrap();
    app.run(OPEN_DRAFT, json!({"compose": compose, "index": 0})).unwrap();
    assert_eq!(opened.lock().unwrap().as_slice(), &[("report.txt".into(), bytes.to_vec())]);
    assert!(!app.session.composing_by_id(compose).unwrap().dirty);

    // Also exercise an attachment whose bytes still live beside the mailbox during composition.
    let file = &mut app.session.compose.as_mut().unwrap().attachments[0];
    file.data = Arc::default();
    file.kept = Some(reference);
    app.run(OPEN_DRAFT, json!({"compose": compose, "index": 0})).unwrap();
    assert_eq!(opened.lock().unwrap().last(), Some(&("report.txt".into(), bytes.to_vec())));
    assert_eq!(opened.lock().unwrap().len(), 2);
}

#[test]
fn opening_respects_file_checks_and_confirmation_cannot_open_a_replacement() {
    use mailcraft_engine::agent::{Access, access};

    let mut app = app();
    let opened = record_open(&mut app);
    let compose = draft(&mut app, "Safe opening", "b3JpZ2luYWw=");
    let params = json!({"compose": compose, "index": 0});
    app.session.compose.as_mut().unwrap().attachments[0].name = "program.exe".into();
    assert!(app.run(OPEN_DRAFT, params.clone()).is_err());
    app.session.compose.as_mut().unwrap().attachments[0].name = "notes.md".into();
    assert!(app.execute(OPEN_DRAFT, params.clone()).is_err(), "the script route needs an explicit confirmation too");
    assert_eq!(app.run(OPEN_DRAFT, params.clone()).unwrap()["pending"], "confirm");
    let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = app.dialog.take() else { panic!("expected an attachment warning") };
    let confirmed = crate::dialogs::answered(&params, &ask.with, None);
    let file = &mut app.session.compose.as_mut().unwrap().attachments[0];
    *file = Attachment::new("notes.md", "text/markdown", b"changed!".to_vec());
    assert!(app.run(OPEN_DRAFT, confirmed).is_err());
    assert!(opened.lock().unwrap().is_empty());
    assert_eq!(access(OPEN_DRAFT), Some(Access::Never));

    app.run(OPEN_DRAFT, params.clone()).unwrap();
    let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = app.dialog.take() else { panic!("expected a fresh warning") };
    app.run(OPEN_DRAFT, crate::dialogs::answered(&params, &ask.with, None)).unwrap();
    assert_eq!(opened.lock().unwrap().as_slice(), &[("notes.md".into(), b"changed!".to_vec())]);
}

#[test]
fn the_browser_downloads_the_draft_copy_and_an_invalid_draft_never_falls_back() {
    let mut app = app();
    let downloaded: Opened = Default::default();
    let sink = downloaded.clone();
    app.services.download = Some(Box::new(move |name, bytes| {
        sink.lock().unwrap().push((name.into(), bytes.to_vec()));
        Ok(())
    }));
    let compose = draft(&mut app, "Browser draft", "Y29weQ==");
    assert!(app.execute(OPEN_DRAFT, json!({"index": 0})).is_err());
    assert!(app.execute(OPEN_DRAFT, json!({"compose": compose + 1, "index": 0})).is_err());
    assert!(app.execute(OPEN_DRAFT, json!({"compose": compose, "index": 1})).is_err());
    assert!(downloaded.lock().unwrap().is_empty());
    assert_eq!(app.execute(OPEN_DRAFT, json!({"compose": compose, "index": 0})).unwrap()["downloaded"], true);
    assert_eq!(downloaded.lock().unwrap().as_slice(), &[("report.txt".into(), b"copy".to_vec())]);
}
