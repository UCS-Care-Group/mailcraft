// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Import HTML…'s guards (File › Signatures, [`crate::signature_import`]), each its own test,
//! through the page's own buttons and the host's picker as the owner's tracer drives them
//! (`signatures::html_import_tests`): answers that belong to no request, or to a request whose
//! window has moved on, change nothing; the caps hold, atomically; a busy host leaves nothing
//! waiting; an answer still outstanding keeps its place when the page's job is cancelled;
//! cancellations are typed and can be retried; and the desktop's reader is bounded and admitted
//! one at a time.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use mailcraft_engine::Session;
use mailcraft_model::signature::{MAX_HTML, MAX_PICTURE_BYTES};
use serde_json::json;

use super::{ASSET, HTML};
use crate::{Delivered, FileDialogRequest, Inbox, MailApp, Services};

const NOW: i64 = 1_791_993_600;
const SIZE: (f32, f32) = (1280.0, 900.0);

/// A document naming two pictures.
const TWO: &str = r#"<div><img src="a/logo.png" alt="a"><img src="b/logo.png" alt="b"></div>"#;
/// A document naming three pictures, for the cap the pictures share.
const THREE: &str = r#"<p><img src="a.png" alt="a"><img src="b.png" alt="b"><img src="c.png" alt="c"></p>"#;
/// A one-pixel PNG (the tracer's light fixture).
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

type Picks = Arc<Mutex<Vec<(String, Option<u64>)>>>;

fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.execute("signature.set", json!({"name": "Old", "text": "Old words", "default": true})).expect("a signature");
    app.execute("ui.backstage", json!({"value": true, "page": "signatures"})).expect("File › Signatures");
    app
}

fn harness(app: MailApp) -> Harness<'static, MailApp> {
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(SIZE.0, SIZE.1)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    h
}

/// File › Signatures with a browser-like picker that records what it was asked for; files come
/// in through `picked`.
fn window() -> (Harness<'static, MailApp>, Picks, Inbox) {
    let mut app = app();
    let picks: Picks = Default::default();
    let log = picks.clone();
    app.services.open_async =
        Some(Box::new(move |purpose, asking| log.lock().unwrap_or_else(|e| e.into_inner()).push((purpose.to_string(), asking))));
    let picked: Inbox = Default::default();
    app.services.picked = Some(picked.clone());
    (harness(app), picks, picked)
}

/// What a desktop host's dialog was asked for, and the answer channels it keeps open.
type Dialogs = Arc<Mutex<Vec<(FileDialogRequest, Sender<Option<Vec<String>>>)>>>;

/// File › Signatures on a desktop-like host: native dialogs that stay open until the test answers.
fn native_window() -> (Harness<'static, MailApp>, Dialogs) {
    let mut app = app();
    let dialogs: Dialogs = Default::default();
    let log = dialogs.clone();
    app.services.file_dialog = Some(Box::new(move |req| {
        let (tx, rx) = std::sync::mpsc::channel();
        log.lock().unwrap_or_else(|e| e.into_inner()).push((req, tx));
        rx
    }));
    (harness(app), dialogs)
}

fn press(h: &mut Harness<'static, MailApp>, key: &str, id: u64) {
    let at = h.state().hit.rect(key, id).unwrap_or_else(|| panic!("no `{key}` {id} drawn")).center();
    h.event(egui::Event::PointerMoved(at));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
}

fn asked(picks: &Picks) -> Vec<(String, Option<u64>)> {
    picks.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The token the picker was last asked with.
fn last_ticket(picks: &Picks) -> Option<u64> {
    asked(picks).last().and_then(|a| a.1)
}

fn deliver(h: &mut Harness<'static, MailApp>, picked: &Inbox, d: Delivered) {
    picked.lock().unwrap_or_else(|e| e.into_inner()).push(d);
    h.run_steps(3);
}

/// Import HTML… pressed and `html` delivered for it.
fn staged(h: &mut Harness<'static, MailApp>, picks: &Picks, picked: &Inbox, html: &str) {
    press(h, "signatureHtmlImport", 0);
    let ticket = last_ticket(picks);
    assert!(ticket.is_some(), "the document was asked for with no token: {:?}", asked(picks));
    deliver(h, picked, Delivered::picked("doc.html".into(), html.as_bytes().to_vec(), HTML, ticket, 1));
}

/// Row `slot`'s Choose image… pressed and `bytes` delivered for it.
fn chose(h: &mut Harness<'static, MailApp>, picks: &Picks, picked: &Inbox, slot: u64, bytes: &[u8]) {
    press(h, "signatureHtmlImportAsset", slot);
    deliver(h, picked, Delivered::picked("logo.png".into(), bytes.to_vec(), ASSET, last_ticket(picks), 2));
}

/// The rows of the job on the page: which have a file, and how many bytes.
fn rows(h: &Harness<'static, MailApp>) -> Option<Vec<Option<usize>>> {
    h.state().signature_import.job.as_ref().map(|j| j.slots.iter().map(|s| s.picked.as_ref().map(|(_, b)| b.len())).collect())
}

fn signatures(h: &Harness<'static, MailApp>) -> Vec<String> {
    h.state().session.mailbox.signatures.iter().map(|s| s.name.clone()).collect()
}

fn note(h: &Harness<'static, MailApp>) -> String {
    h.state().signature_import.note.clone().unwrap_or_default()
}

/// Import HTML… pressed: the picker is asked once for the document, with a token.
#[test]
fn import_html_asks_the_picker_for_the_document() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, _picked) = window();
    press(&mut h, "signatureHtmlImport", 0);
    let asked = asked(&picks);
    assert_eq!(asked.len(), 1, "asked {asked:?}");
    assert_eq!(asked.first().map(|a| a.0.as_str()), Some(HTML));
    assert!(asked.first().is_some_and(|a| a.1.is_some()), "no token: {asked:?}");
}

/// A document answered under another token, or for another purpose, is nobody's: no job. The
/// one under the request's own token is taken, once.
#[test]
fn a_document_under_another_token_or_purpose_is_ignored() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, picked) = window();
    press(&mut h, "signatureHtmlImport", 0);
    let ticket = last_ticket(&picks).expect("a token");
    deliver(&mut h, &picked, Delivered::picked("doc.html".into(), TWO.as_bytes().to_vec(), HTML, Some(ticket + 1), 1));
    deliver(&mut h, &picked, Delivered::picked("doc.html".into(), TWO.as_bytes().to_vec(), HTML, None, 1));
    deliver(&mut h, &picked, Delivered::picked("doc.html".into(), TWO.as_bytes().to_vec(), ASSET, Some(ticket), 1));
    assert!(rows(&h).is_none(), "an answer to no request made a job");
    deliver(&mut h, &picked, Delivered::picked("doc.html".into(), TWO.as_bytes().to_vec(), HTML, Some(ticket), 1));
    assert_eq!(rows(&h), Some(vec![None, None]), "the request's own answer made no job");
}

/// A picture delivered twice under its token fills its own row once, and never the next row.
#[test]
fn a_duplicate_picture_answer_fills_no_other_row() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, picked) = window();
    staged(&mut h, &picks, &picked, TWO);
    chose(&mut h, &picks, &picked, 0, PNG);
    let ticket = last_ticket(&picks);
    deliver(&mut h, &picked, Delivered::picked("other.png".into(), vec![7; 9], ASSET, ticket, 3));
    assert_eq!(rows(&h), Some(vec![Some(PNG.len()), None]), "a repeated answer filled or replaced a row");
    assert_eq!(asked(&picks).len(), 2, "an answer asked for another file");
}

/// A document that arrives once the editor holds another signature is not staged.
#[test]
fn a_document_for_another_editor_is_ignored() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, picked) = window();
    press(&mut h, "signatureHtmlImport", 0);
    let ticket = last_ticket(&picks);
    h.state_mut().execute("signature.set", json!({"name": "Other", "text": "Other words"})).expect("another signature");
    let before = h.state().session.signature_edit.as_ref().map(|e| e.signature.name.clone());
    h.state_mut().execute("signature.open", json!({"name": "Other"})).expect("the editor opens another");
    assert_ne!(h.state().session.signature_edit.as_ref().map(|e| e.signature.name.clone()), before, "the fixture's editor did not change");
    deliver(&mut h, &picked, Delivered::picked("doc.html".into(), TWO.as_bytes().to_vec(), HTML, ticket, 1));
    assert!(rows(&h).is_none(), "a document asked for in another editor's time was staged");
}

/// A picture that arrives once another mailbox is open fills nothing.
#[test]
fn a_picture_for_another_mailbox_is_ignored() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, picked) = window();
    staged(&mut h, &picks, &picked, TWO);
    press(&mut h, "signatureHtmlImportAsset", 0);
    let ticket = last_ticket(&picks);
    let openings = h.state().session.openings();
    h.state_mut().execute("file.new", json!({})).expect("another mailbox");
    assert_ne!(h.state().session.openings(), openings, "the fixture did not open another mailbox");
    deliver(&mut h, &picked, Delivered::picked("logo.png".into(), PNG.to_vec(), ASSET, ticket, 2));
    assert!(rows(&h).is_none_or(|r| r.iter().all(Option::is_none)), "a picture asked for in another mailbox's time was staged");
}

/// Import pressed once the editor holds another signature stores nothing.
#[test]
fn import_after_the_editor_changed_stores_nothing() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, picked) = window();
    staged(&mut h, &picks, &picked, TWO);
    chose(&mut h, &picks, &picked, 0, PNG);
    chose(&mut h, &picks, &picked, 1, PNG);
    assert_eq!(rows(&h), Some(vec![Some(PNG.len()), Some(PNG.len())]), "the fixture's rows are not both filled");
    h.state_mut().execute("signature.set", json!({"name": "Other", "text": "Other words"})).expect("another signature");
    h.state_mut().execute("signature.open", json!({"name": "Other"})).expect("the editor opens another");
    let before = signatures(&h);
    if h.state().hit.rect("signatureHtmlImportCommit", 0).is_some() {
        press(&mut h, "signatureHtmlImportCommit", 0);
    }
    assert_eq!(signatures(&h), before, "Import stored a signature for a window that had moved on");
}

/// A document of exactly the most HTML a signature may have is staged.
#[test]
fn html_at_the_cap_is_staged() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, picked) = window();
    let html = format!("<p>x</p>{}", " ".repeat(MAX_HTML - 8));
    assert_eq!(html.len(), MAX_HTML);
    staged(&mut h, &picks, &picked, &html);
    assert_eq!(rows(&h), Some(vec![]), "a document at the cap was not staged");
}

/// One byte more is refused, and said as the cap.
#[test]
fn html_past_the_cap_is_refused_as_too_large() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, picked) = window();
    let html = format!("<p>x</p>{}", " ".repeat(MAX_HTML - 7));
    staged(&mut h, &picks, &picked, &html);
    assert!(rows(&h).is_none(), "a document past the cap was staged");
    assert!(note(&h).contains("64 KB"), "the page did not say the document is too large: {:?}", note(&h));
}

/// A document that is not UTF-8 is refused and said so.
#[test]
fn html_that_is_not_utf8_is_refused() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, picked) = window();
    press(&mut h, "signatureHtmlImport", 0);
    deliver(&mut h, &picked, Delivered::picked("doc.html".into(), vec![b'<', b'p', b'>', 0xff, 0xfe], HTML, last_ticket(&picks), 1));
    assert!(rows(&h).is_none(), "a document that is not UTF-8 was staged");
    assert!(note(&h).contains("UTF-8"), "the page did not say the document is not UTF-8: {:?}", note(&h));
}

/// A picture of exactly the cap fills its row; one byte more leaves the rows as they were.
#[test]
fn a_picture_past_its_cap_leaves_its_row_as_it_was() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, picked) = window();
    staged(&mut h, &picks, &picked, TWO);
    chose(&mut h, &picks, &picked, 0, &vec![1; MAX_PICTURE_BYTES]);
    assert_eq!(rows(&h), Some(vec![Some(MAX_PICTURE_BYTES), None]), "a picture at the cap was not taken");
    chose(&mut h, &picks, &picked, 0, &vec![2; MAX_PICTURE_BYTES + 1]);
    chose(&mut h, &picks, &picked, 1, &vec![3; MAX_PICTURE_BYTES + 1]);
    assert_eq!(rows(&h), Some(vec![Some(MAX_PICTURE_BYTES), None]), "a picture past the cap changed a row");
    assert!(note(&h).contains("256 KB"), "the page did not say the picture is too large: {:?}", note(&h));
}

/// The pictures together stay within their shared cap: the one that would pass it is refused and
/// changes nothing, and a row's own old file is not counted against its replacement.
#[test]
fn the_pictures_together_stay_within_their_cap() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, picked) = window();
    staged(&mut h, &picks, &picked, THREE);
    let full = vec![1; MAX_PICTURE_BYTES];
    chose(&mut h, &picks, &picked, 0, &full);
    chose(&mut h, &picks, &picked, 1, &full);
    chose(&mut h, &picks, &picked, 2, &[9]);
    assert_eq!(rows(&h), Some(vec![Some(MAX_PICTURE_BYTES), Some(MAX_PICTURE_BYTES), None]), "a picture past the shared cap was taken");
    assert!(note(&h).contains("512 KB"), "the page did not say the pictures are too large together: {:?}", note(&h));
    chose(&mut h, &picks, &picked, 1, &vec![2; MAX_PICTURE_BYTES]);
    assert_eq!(rows(&h), Some(vec![Some(MAX_PICTURE_BYTES), Some(MAX_PICTURE_BYTES), None]), "a replacement within the cap was refused");
    assert!(note(&h).is_empty(), "a replacement within the cap was said to be refused: {:?}", note(&h));
}

/// A document that names no pictures still waits for the owner's Import, which stores it once.
#[test]
fn a_document_without_pictures_waits_for_import() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, picked) = window();
    let before = signatures(&h).len();
    staged(&mut h, &picks, &picked, "<p>Northwind</p>");
    h.run_steps(4);
    assert_eq!(signatures(&h).len(), before, "the document was imported before the owner pressed Import");
    press(&mut h, "signatureHtmlImportCommit", 0);
    assert_eq!(signatures(&h).len(), before + 1, "Import stored no signature, or more than one");
    assert!(rows(&h).is_none(), "the job stayed after the import");
}

/// Cancel lets go of the job, but not of a pick still outstanding: Import HTML… asks nothing
/// until its answer has come (and is ignored), then asks again.
#[test]
fn cancel_keeps_an_outstanding_pick_until_it_is_answered() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, picked) = window();
    staged(&mut h, &picks, &picked, TWO);
    press(&mut h, "signatureHtmlImportAsset", 0);
    let ticket = last_ticket(&picks);
    press(&mut h, "signatureHtmlImportCancel", 0);
    assert!(rows(&h).is_none(), "Cancel kept the job");
    press(&mut h, "signatureHtmlImport", 0);
    assert_eq!(asked(&picks).len(), 2, "a second pick was asked for while one was outstanding: {:?}", asked(&picks));
    deliver(&mut h, &picked, Delivered::picked("logo.png".into(), PNG.to_vec(), ASSET, ticket, 2));
    assert!(rows(&h).is_none(), "the outstanding answer made a job");
    press(&mut h, "signatureHtmlImport", 0);
    assert_eq!(asked(&picks).len(), 3, "Import HTML… asked nothing once the outstanding pick was answered");
    assert_eq!(asked(&picks).last().map(|a| a.0.clone()).as_deref(), Some(HTML));
}

/// A desktop host with another file dialog open: Import HTML… shows none and leaves no request
/// waiting; once that dialog is answered, Import HTML… asks for the document.
#[test]
fn a_busy_native_dialog_leaves_no_request_waiting() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, dialogs, _reads) = native_reader_window();
    let _ = h.state_mut().run("file.import", json!({}));
    assert!(h.state().file_dialog.is_some(), "the fixture's other dialog is not open");
    press(&mut h, "signatureHtmlImport", 0);
    assert_eq!(dialogs.lock().unwrap_or_else(|e| e.into_inner()).len(), 1, "a second dialog was asked for");
    assert!(h.state().signature_import.asked.is_none(), "a request waits for a dialog that was never shown");
    assert!(!note(&h).is_empty(), "the page said nothing of the dialog already open");
    if let Some((_, tx)) = dialogs.lock().unwrap_or_else(|e| e.into_inner()).first() {
        let _ = tx.send(None);
    }
    h.run_steps(3);
    assert!(h.state().file_dialog.is_none(), "the fixture's other dialog is still open");
    press(&mut h, "signatureHtmlImport", 0);
    let shown: Vec<FileDialogRequest> = dialogs.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|d| d.0.clone()).collect();
    assert_eq!(shown.get(1), Some(&FileDialogRequest::Open { purpose: HTML.into() }), "Import HTML… asked for {shown:?}");
}

/// What the desktop host's reader was asked to read, and the answer channels it keeps open.
type Reads = Arc<Mutex<Vec<Sender<Result<Vec<u8>, crate::Unread>>>>>;

/// File › Signatures on a desktop-like host with native dialogs and a reader that answers only
/// when the test says.
fn native_reader_window() -> (Harness<'static, MailApp>, Dialogs, Reads) {
    let (mut h, dialogs) = native_window();
    let reads: Reads = Default::default();
    let log = reads.clone();
    h.state_mut().services.read_signature_file = Some(Box::new(move |_read: crate::files::SignatureRead| {
        let (tx, rx) = std::sync::mpsc::channel();
        log.lock().unwrap_or_else(|e| e.into_inner()).push(tx);
        rx
    }));
    (h, dialogs, reads)
}

/// The last native dialog answered with `path` (or cancelled with `None`).
fn answer_dialog(h: &mut Harness<'static, MailApp>, dialogs: &Dialogs, path: Option<&str>) {
    if let Some((_, tx)) = dialogs.lock().unwrap_or_else(|e| e.into_inner()).last() {
        let _ = tx.send(path.map(|p| vec![p.to_string()]));
    }
    h.run_steps(3);
}

fn shown(dialogs: &Dialogs) -> usize {
    dialogs.lock().unwrap_or_else(|e| e.into_inner()).len()
}

fn read_count(reads: &Reads) -> usize {
    reads.lock().unwrap_or_else(|e| e.into_inner()).len()
}

/// The browser's picker cancelled for a row: the job, its document and its other rows stay, and
/// the row can be chosen again. Cancelled for the document: no job, and Import HTML… asks again.
#[test]
fn a_cancelled_browser_pick_keeps_the_job_and_can_be_retried() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, picked) = window();
    staged(&mut h, &picks, &picked, TWO);
    chose(&mut h, &picks, &picked, 0, PNG);
    press(&mut h, "signatureHtmlImportAsset", 1);
    deliver(&mut h, &picked, Delivered::cancelled(ASSET, last_ticket(&picks), 3));
    assert_eq!(rows(&h), Some(vec![Some(PNG.len()), None]), "a cancelled pick changed the job");
    assert!(h.state().signature_import.asked.is_none(), "the cancelled pick still waits");
    assert!(note(&h).is_empty(), "a cancellation was said as a failure: {:?}", note(&h));
    chose(&mut h, &picks, &picked, 1, PNG);
    assert_eq!(rows(&h), Some(vec![Some(PNG.len()), Some(PNG.len())]), "the row could not be chosen again");

    let (mut h, picks, picked) = window();
    press(&mut h, "signatureHtmlImport", 0);
    deliver(&mut h, &picked, Delivered::cancelled(HTML, last_ticket(&picks), 1));
    assert!(rows(&h).is_none(), "a cancelled document made a job");
    press(&mut h, "signatureHtmlImport", 0);
    assert_eq!(asked(&picks).len(), 2, "Import HTML… asked nothing after a cancelled pick");
}

/// A cancellation delivered for another purpose (a file to attach) is said nowhere and changes
/// nothing: only a signature's pick is answered that way.
#[test]
fn a_cancellation_for_another_purpose_changes_nothing() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, _picks, picked) = window();
    let (said, backstage) = (h.state().status_msg.clone(), h.state().ui.backstage);
    deliver(&mut h, &picked, Delivered::cancelled("attach", Some(1), 1));
    assert_eq!(h.state().status_msg, said, "a cancellation for a file to attach was said");
    assert_eq!(h.state().ui.backstage, backstage, "a cancellation for a file to attach closed File");
}

/// A desktop dialog cancelled for the document: the request is let go of, the dialog's answer is
/// still `cancelled`, and Import HTML… can ask again.
#[test]
fn a_cancelled_native_dialog_lets_go_of_its_request() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, dialogs, reads) = native_reader_window();
    press(&mut h, "signatureHtmlImport", 0);
    assert_eq!(shown(&dialogs), 1, "Import HTML… showed no dialog");
    if let Some((_, tx)) = dialogs.lock().unwrap_or_else(|e| e.into_inner()).last() {
        let _ = tx.send(None);
    }
    let answer = crate::files::poll(h.state_mut());
    assert_eq!(answer, Some(Ok(json!({"cancelled": true}))), "a cancelled dialog answered otherwise");
    assert!(h.state().signature_import.asked.is_none(), "the cancelled request still waits");
    assert_eq!(read_count(&reads), 0, "a cancelled dialog read something");
    h.run_steps(2);
    press(&mut h, "signatureHtmlImport", 0);
    assert_eq!(shown(&dialogs), 2, "Import HTML… asked nothing after a cancelled dialog");
}

/// The document picked in a desktop dialog is read by the host's reader, off the window, never by
/// the window; its answer makes the job, and one reader at a time is admitted.
#[test]
fn a_native_pick_is_read_by_the_host_reader_one_at_a_time() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, dialogs, reads) = native_reader_window();
    press(&mut h, "signatureHtmlImport", 0);
    answer_dialog(&mut h, &dialogs, Some("/synthetic/never-read/doc.html"));
    assert_eq!(read_count(&reads), 1, "the picked document was not handed to the host's reader");
    assert!(rows(&h).is_none(), "the job was made before the reader answered");
    press(&mut h, "signatureHtmlImport", 0);
    assert_eq!(shown(&dialogs), 1, "a second dialog was shown while a file was being read");
    if let Some(tx) = reads.lock().unwrap_or_else(|e| e.into_inner()).first() {
        let _ = tx.send(Ok(TWO.as_bytes().to_vec()));
    }
    h.run_steps(3);
    assert_eq!(rows(&h), Some(vec![None, None]), "the reader's answer made no job");
    assert!(h.state().signature_import.reading.is_none(), "the answered reader still holds its place");
}

/// Cancel lets go of the job while a picture is being read, but the reader keeps its place until
/// it answers or goes away: no second reader or dialog is started meanwhile; then one is.
#[test]
fn a_cancelled_job_keeps_its_reader_until_it_is_done() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, dialogs, reads) = native_reader_window();
    press(&mut h, "signatureHtmlImport", 0);
    answer_dialog(&mut h, &dialogs, Some("/synthetic/never-read/doc.html"));
    if let Some(tx) = reads.lock().unwrap_or_else(|e| e.into_inner()).first() {
        let _ = tx.send(Ok(TWO.as_bytes().to_vec()));
    }
    h.run_steps(3);
    press(&mut h, "signatureHtmlImportAsset", 0);
    answer_dialog(&mut h, &dialogs, Some("/synthetic/never-read/logo.png"));
    assert_eq!(read_count(&reads), 2, "the picked picture was not handed to the host's reader");
    press(&mut h, "signatureHtmlImportCancel", 0);
    assert!(rows(&h).is_none(), "Cancel kept the job");
    press(&mut h, "signatureHtmlImport", 0);
    assert_eq!((shown(&dialogs), read_count(&reads)), (2, 2), "something was started while the cancelled job's reader was still reading");
    // The reader goes away without an answer: its place is free again.
    reads.lock().unwrap_or_else(|e| e.into_inner()).clear();
    h.run_steps(3);
    assert!(h.state().signature_import.reading.is_none(), "a reader that went away still holds its place");
    press(&mut h, "signatureHtmlImport", 0);
    assert_eq!(shown(&dialogs), 3, "Import HTML… asked nothing once the reader was done");
}

/// An exclusively created folder of this test's own, under the run's temporary folder.
fn own_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sigpick-{name}-{}", std::process::id()));
    std::fs::create_dir(&dir).expect("an exclusively created folder");
    dir
}

/// The document's reader takes exactly the most a signature's HTML may have and refuses one byte
/// more; a picture's reader is held to a picture's cap. A folder is no file.
#[test]
fn the_desktop_reader_holds_each_file_to_its_cap() {
    let dir = own_dir("caps");
    let at = dir.join("at.html");
    let over = dir.join("over.html");
    std::fs::write(&at, vec![b'a'; MAX_HTML]).expect("a fixture");
    std::fs::write(&over, vec![b'a'; MAX_HTML + 1]).expect("a fixture");
    let picture = dir.join("over.png");
    std::fs::write(&picture, vec![1; MAX_PICTURE_BYTES + 1]).expect("a fixture");
    let path = |p: &std::path::Path| p.to_string_lossy().into_owned();
    assert_eq!(crate::files::SignatureRead::document(&path(&at)).read().map(|b| b.len()), Ok(MAX_HTML));
    assert_eq!(crate::files::SignatureRead::document(&path(&over)).read(), Err(crate::Unread::TooLarge));
    assert_eq!(crate::files::SignatureRead::picture(&path(&picture)).read(), Err(crate::Unread::TooLarge));
    assert_eq!(crate::files::SignatureRead::picture(&path(&at)).read().map(|b| b.len()), Ok(MAX_HTML));
    assert!(matches!(crate::files::SignatureRead::document(&path(&dir)).read(), Err(crate::Unread::Failed(_))), "a folder was read");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A FIFO named as the document is refused at once, not waited on.
#[cfg(unix)]
#[test]
fn the_desktop_reader_refuses_a_fifo_without_waiting() {
    let dir = own_dir("fifo");
    let fifo = dir.join("doc.html");
    // (Made with the system's own `mkfifo`: a precondition, never skipped quietly.)
    let made = std::process::Command::new("mkfifo").arg(&fifo).status();
    assert!(made.as_ref().is_ok_and(|s| s.success()), "the fixture's FIFO could not be made: {made:?}");
    let read = crate::files::SignatureRead::document(&fifo.to_string_lossy());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(read.read());
    });
    let answer = rx.recv_timeout(std::time::Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(matches!(answer, Ok(Err(crate::Unread::Failed(_)))), "a FIFO was not refused at once: {answer:?}");
}

/// A one-pixel PNG that is not [`PNG`]: the owner tracer's other valid fixture
/// (`signatures::html_import_tests`), so a replacement can be told from the file it replaced.
const DARK_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x56, 0xc7, 0x2f, 0x0d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// The whole mailbox and the signature open in the editor, as they are now.
fn kept(h: &Harness<'static, MailApp>) -> (mailcraft_model::Mailbox, Option<mailcraft_engine::SignatureEdit>) {
    let s = &h.state().session;
    (s.mailbox.clone(), s.signature_edit.clone())
}

/// The last reader the host started answers `answer`, and the window's frames run.
fn answer_last_read(h: &mut Harness<'static, MailApp>, reads: &Reads, answer: Result<Vec<u8>, crate::Unread>) {
    if let Some(tx) = reads.lock().unwrap_or_else(|e| e.into_inner()).last() {
        let _ = tx.send(answer);
    }
    h.run_steps(3);
}

/// On the desktop: Import HTML… pressed, `doc.html` picked in the dialog and read as `html`.
fn native_staged(h: &mut Harness<'static, MailApp>, dialogs: &Dialogs, reads: &Reads, html: &str) {
    press(h, "signatureHtmlImport", 0);
    answer_dialog(h, dialogs, Some("/synthetic/never-read/doc.html"));
    answer_last_read(h, reads, Ok(html.as_bytes().to_vec()));
}

/// On the desktop: row `slot`'s Choose image…, a file picked in the dialog and read as `bytes`.
fn native_chose(h: &mut Harness<'static, MailApp>, dialogs: &Dialogs, reads: &Reads, slot: u64, bytes: &[u8]) {
    press(h, "signatureHtmlImportAsset", slot);
    answer_dialog(h, dialogs, Some("/synthetic/never-read/logo.png"));
    answer_last_read(h, reads, Ok(bytes.to_vec()));
}

/// Exactly one signature was stored since `before`, named `doc` as the page offered it: the one
/// the constructor makes of `html` with `pictures`, one file per named resource in order; the
/// mailbox is `before`'s with it appended, and the editor is as it was.
fn stored_exactly(
    h: &Harness<'static, MailApp>,
    before: &(mailcraft_model::Mailbox, Option<mailcraft_engine::SignatureEdit>),
    html: &str,
    pictures: &[&[u8]],
) {
    let resources = mailcraft_mime_model::signature_import::required_assets(html).expect("the fixture's document is one the importer reads");
    assert_eq!(resources.len(), pictures.len(), "the fixture gives {} files for {resources:?}", pictures.len());
    let assets: Vec<mailcraft_mime_model::signature_import::Asset> = resources
        .iter()
        .zip(pictures)
        .map(|(name, data)| mailcraft_mime_model::signature_import::Asset { name: name.clone(), data: data.to_vec() })
        .collect();
    let produced = mailcraft_mime_model::signature_import::import("doc", html, &assets).expect("the constructor makes a signature of the fixture");
    let stored =
        h.state().session.mailbox.signature("doc").cloned().unwrap_or_else(|| panic!("Import stored no signature named `doc`: {:?}", signatures(h)));
    assert_eq!(stored.html(), produced.html(), "the stored document is not the one made of the files chosen last");
    assert_eq!(stored.pictures(), produced.pictures(), "the stored pictures are not the files chosen last");
    let mut expected = before.0.clone();
    expected.signatures.push(stored);
    assert!(h.state().session.mailbox == expected, "Import changed the mailbox beyond appending the one new signature");
    assert!(h.state().session.signature_edit == before.1, "Import changed the signature open in the editor");
}

/// A ready job, and Import HTML… pressed again on the desktop: while the new document is being
/// read, the owner presses Cancel. The reader keeps its place until it answers; its late answer, a
/// valid document, makes no job and changes nothing; then Import HTML… can start again.
#[test]
fn a_document_read_after_cancel_makes_no_job() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, dialogs, reads) = native_reader_window();
    native_staged(&mut h, &dialogs, &reads, "<p>Old document</p>");
    assert_eq!(rows(&h), Some(vec![]), "the fixture's old job is not staged");
    assert!(h.state().signature_import.job.as_ref().is_some_and(super::Job::ready), "the fixture's old job is not ready");
    press(&mut h, "signatureHtmlImport", 0);
    answer_dialog(&mut h, &dialogs, Some("/synthetic/never-read/new.html"));
    assert_eq!(read_count(&reads), 2, "the new document was not handed to the host's reader");
    let before = kept(&h);
    press(&mut h, "signatureHtmlImportCancel", 0);
    assert!(rows(&h).is_none(), "Cancel kept the job");
    h.run_steps(2);
    assert!(h.state().signature_import.reading.is_some(), "the cancelled read gave up its place before it answered");
    press(&mut h, "signatureHtmlImport", 0);
    assert_eq!((shown(&dialogs), read_count(&reads)), (2, 2), "something was started while the cancelled read was still reading");
    answer_last_read(&mut h, &reads, Ok(TWO.as_bytes().to_vec()));
    assert!(rows(&h).is_none(), "the cancelled read's late document made a job");
    assert!(h.state().signature_import.reading.is_none(), "the answered read still holds its place");
    assert!(kept(&h) == before, "the cancelled read's late document changed the mailbox or the editor");
    press(&mut h, "signatureHtmlImport", 0);
    assert_eq!(shown(&dialogs), 3, "Import HTML… asked nothing once the cancelled read was done");
}

/// The same in the browser: a ready job, Import HTML… pressed again and Cancel pressed while the
/// picker is open. The pick keeps its place until it is answered; its document makes no job.
#[test]
fn a_document_delivered_after_cancel_makes_no_job() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, picked) = window();
    staged(&mut h, &picks, &picked, "<p>Old document</p>");
    assert_eq!(rows(&h), Some(vec![]), "the fixture's old job is not staged");
    press(&mut h, "signatureHtmlImport", 0);
    let ticket = last_ticket(&picks);
    assert_eq!(asked(&picks).len(), 2, "the new document was not asked for");
    let before = kept(&h);
    press(&mut h, "signatureHtmlImportCancel", 0);
    assert!(rows(&h).is_none(), "Cancel kept the job");
    press(&mut h, "signatureHtmlImport", 0);
    assert_eq!(asked(&picks).len(), 2, "a pick was asked for while the cancelled one was outstanding");
    deliver(&mut h, &picked, Delivered::picked("new.html".into(), TWO.as_bytes().to_vec(), HTML, ticket, 2));
    assert!(rows(&h).is_none(), "the cancelled pick's late document made a job");
    assert!(kept(&h) == before, "the cancelled pick's late document changed the mailbox or the editor");
    press(&mut h, "signatureHtmlImport", 0);
    assert_eq!(asked(&picks).len(), 3, "Import HTML… asked nothing once the cancelled pick was answered");
}

/// On the desktop, Cancel pressed while the new document's dialog is still open: the path it
/// answers afterwards is handed to no reader, and Import HTML… can ask again.
#[test]
fn a_path_answered_after_cancel_starts_no_reader() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, dialogs, reads) = native_reader_window();
    native_staged(&mut h, &dialogs, &reads, "<p>Old document</p>");
    assert_eq!(rows(&h), Some(vec![]), "the fixture's old job is not staged");
    press(&mut h, "signatureHtmlImport", 0);
    assert_eq!(shown(&dialogs), 2, "the new document's dialog was not shown");
    press(&mut h, "signatureHtmlImportCancel", 0);
    assert!(rows(&h).is_none(), "Cancel kept the job");
    answer_dialog(&mut h, &dialogs, Some("/synthetic/never-read/new.html"));
    assert_eq!(read_count(&reads), 1, "a path answered after Cancel was handed to the host's reader");
    assert!(h.state().signature_import.asked.is_none() && h.state().signature_import.reading.is_none(), "the cancelled request still holds a place");
    assert!(rows(&h).is_none(), "the cancelled request made a job");
    press(&mut h, "signatureHtmlImport", 0);
    assert_eq!(shown(&dialogs), 3, "Import HTML… asked nothing once the cancelled dialog was answered");
}

/// A ready job whose first row's file is being replaced: Import stores nothing until the new file
/// has come, so the old one is never stored in its place.
#[test]
fn import_waits_while_a_picture_is_being_replaced() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, picked) = window();
    staged(&mut h, &picks, &picked, TWO);
    chose(&mut h, &picks, &picked, 0, PNG);
    chose(&mut h, &picks, &picked, 1, PNG);
    assert_eq!(rows(&h), Some(vec![Some(PNG.len()), Some(PNG.len())]), "the fixture's rows are not both filled");
    press(&mut h, "signatureHtmlImportAsset", 0);
    assert_eq!(asked(&picks).len(), 4, "the replacement was not asked for");
    let before = kept(&h);
    press(&mut h, "signatureHtmlImportCommit", 0);
    assert!(kept(&h) == before, "Import stored the job while a replacement picture was still outstanding");
    assert_eq!(rows(&h), Some(vec![Some(PNG.len()), Some(PNG.len())]), "Import let go of the job while a picture was outstanding");
}

/// A ready job and a second document asked for: Import stores nothing while it is outstanding.
/// Once that pick is cancelled, the job is ready again and Import stores it as it was.
#[test]
fn import_waits_while_another_document_is_being_chosen() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, picked) = window();
    staged(&mut h, &picks, &picked, TWO);
    chose(&mut h, &picks, &picked, 0, PNG);
    chose(&mut h, &picks, &picked, 1, DARK_PNG);
    press(&mut h, "signatureHtmlImport", 0);
    let ticket = last_ticket(&picks);
    assert_eq!(asked(&picks).last().map(|a| a.0.clone()).as_deref(), Some(HTML), "the second document was not asked for");
    let before = kept(&h);
    press(&mut h, "signatureHtmlImportCommit", 0);
    assert!(kept(&h) == before, "Import stored the job while another document was outstanding");
    deliver(&mut h, &picked, Delivered::cancelled(HTML, ticket, 4));
    assert!(h.state().signature_import.asked.is_none(), "the cancelled document still waits");
    press(&mut h, "signatureHtmlImportCommit", 0);
    stored_exactly(&h, &before, TWO, &[PNG, DARK_PNG]);
}

/// The import's own backstop, called as the page calls it: while a replacement picture is being
/// read on the desktop, nothing is handed to `signature.import`; once the reader has answered,
/// the new file is stored, not the old.
#[test]
fn commit_waits_for_a_picture_being_read() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, dialogs, reads) = native_reader_window();
    native_staged(&mut h, &dialogs, &reads, TWO);
    native_chose(&mut h, &dialogs, &reads, 0, PNG);
    native_chose(&mut h, &dialogs, &reads, 1, PNG);
    assert_eq!(rows(&h), Some(vec![Some(PNG.len()), Some(PNG.len())]), "the fixture's rows are not both filled");
    press(&mut h, "signatureHtmlImportAsset", 0);
    answer_dialog(&mut h, &dialogs, Some("/synthetic/never-read/dark.png"));
    assert_eq!(read_count(&reads), 4, "the replacement was not handed to the host's reader");
    let before = kept(&h);
    super::commit(h.state_mut());
    h.run_steps(2);
    assert!(kept(&h) == before, "the import was handed the old picture while its replacement was being read");
    assert_eq!(rows(&h), Some(vec![Some(PNG.len()), Some(PNG.len())]), "the job changed while its replacement was being read");
    answer_last_read(&mut h, &reads, Ok(DARK_PNG.to_vec()));
    assert_eq!(rows(&h), Some(vec![Some(DARK_PNG.len()), Some(PNG.len())]), "the replacement did not fill its row");
    super::commit(h.state_mut());
    h.run_steps(2);
    stored_exactly(&h, &before, TWO, &[DARK_PNG, PNG]);
}

/// A replacement that has come is what Import stores, never the file it replaced.
#[test]
fn import_stores_the_replacement_once_it_has_come() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, picked) = window();
    staged(&mut h, &picks, &picked, TWO);
    chose(&mut h, &picks, &picked, 0, PNG);
    chose(&mut h, &picks, &picked, 1, PNG);
    chose(&mut h, &picks, &picked, 0, DARK_PNG);
    assert_eq!(rows(&h), Some(vec![Some(DARK_PNG.len()), Some(PNG.len())]), "the replacement did not fill its row");
    let before = kept(&h);
    press(&mut h, "signatureHtmlImportCommit", 0);
    stored_exactly(&h, &before, TWO, &[DARK_PNG, PNG]);
}

/// The unchanged job: nothing outstanding, both rows filled, Import stores it once.
#[test]
fn an_unchanged_ready_job_imports_normally() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, picked) = window();
    staged(&mut h, &picks, &picked, TWO);
    chose(&mut h, &picks, &picked, 0, PNG);
    chose(&mut h, &picks, &picked, 1, DARK_PNG);
    let before = kept(&h);
    press(&mut h, "signatureHtmlImportCommit", 0);
    stored_exactly(&h, &before, TWO, &[PNG, DARK_PNG]);
    assert!(rows(&h).is_none(), "the job stayed after the import");
}

/// The first document being read on the desktop, with no job yet: the page offers Cancel. Once
/// pressed (twice: the second changes nothing), the reader keeps its place until it answers, its
/// valid document makes no job, and Import HTML… can start again.
#[test]
fn the_first_document_being_read_can_be_cancelled() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, dialogs, reads) = native_reader_window();
    press(&mut h, "signatureHtmlImport", 0);
    answer_dialog(&mut h, &dialogs, Some("/synthetic/never-read/doc.html"));
    assert_eq!(read_count(&reads), 1, "the document was not handed to the host's reader");
    assert!(rows(&h).is_none(), "the fixture has a job before the reader answered");
    assert!(h.state().hit.rect("signatureHtmlImportCancel", 0).is_some(), "the page offers no Cancel while the first document is being read");
    let before = kept(&h);
    press(&mut h, "signatureHtmlImportCancel", 0);
    h.run_steps(2);
    assert!(h.state().signature_import.reading.is_some(), "the cancelled read gave up its place before it answered");
    if h.state().hit.rect("signatureHtmlImportCancel", 0).is_some() {
        press(&mut h, "signatureHtmlImportCancel", 0);
    }
    press(&mut h, "signatureHtmlImport", 0);
    assert_eq!((shown(&dialogs), read_count(&reads)), (1, 1), "something was started while the cancelled read was still reading");
    answer_last_read(&mut h, &reads, Ok(TWO.as_bytes().to_vec()));
    assert!(rows(&h).is_none(), "the cancelled read's late document made a job");
    assert!(h.state().signature_import.reading.is_none(), "the answered read still holds its place");
    assert!(kept(&h) == before, "the cancelled read's late document changed the mailbox or the editor");
    press(&mut h, "signatureHtmlImport", 0);
    assert_eq!(shown(&dialogs), 2, "Import HTML… asked nothing once the cancelled read was done");
}

/// The first document asked of the browser's picker, with no job yet: the page offers Cancel, and
/// the document delivered after it makes no job.
#[test]
fn the_first_document_pick_can_be_cancelled() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let (mut h, picks, picked) = window();
    press(&mut h, "signatureHtmlImport", 0);
    let ticket = last_ticket(&picks);
    assert!(h.state().hit.rect("signatureHtmlImportCancel", 0).is_some(), "the page offers no Cancel while the first document is being chosen");
    let before = kept(&h);
    press(&mut h, "signatureHtmlImportCancel", 0);
    press(&mut h, "signatureHtmlImport", 0);
    assert_eq!(asked(&picks).len(), 1, "a pick was asked for while the cancelled one was outstanding");
    deliver(&mut h, &picked, Delivered::picked("doc.html".into(), TWO.as_bytes().to_vec(), HTML, ticket, 1));
    assert!(rows(&h).is_none(), "the cancelled pick's late document made a job");
    assert!(kept(&h) == before, "the cancelled pick's late document changed the mailbox or the editor");
    press(&mut h, "signatureHtmlImport", 0);
    assert_eq!(asked(&picks).len(), 2, "Import HTML… asked nothing once the cancelled pick was answered");
}
