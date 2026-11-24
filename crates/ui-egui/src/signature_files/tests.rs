// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! External signature files: local settings, fresh reads, snapshots and fallback (#1878).

use super::*;
use crate::files::base64_lite;
use crate::{Services, UiState};
use mailcraft_engine::{Session, sample};
use mailcraft_model::Id;

const NOW: i64 = 1_791_993_600;

fn app() -> (MailApp, Id) {
    let mut app = MailApp::new(Session::at(sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    let account = app.session.mailbox.default_account().unwrap().id;
    app.execute("signature.set", json!({"name": "Usual", "text": "Usual signature", "default": true, "account": account})).unwrap();
    (app, account)
}

fn configure(app: &mut MailApp, account: Id, path: &Path, enabled: bool) {
    if app.session.mailbox.account(account).unwrap().signature.is_empty() {
        app.execute("signature.set", json!({"name":format!("Usual {account}"),"text":"Usual signature","account":account,"default":true})).unwrap();
    }
    app.execute("ui.signatureFile", json!({"account": account, "path": path, "enabled": enabled})).unwrap();
}

fn write(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn compose_text(app: &mut MailApp, account: Id) -> String {
    app.execute("mail.new", json!({"account": account})).unwrap();
    app.session.compose.as_ref().unwrap().body.text().to_plain()
}

#[test]
fn signature_file_is_read_again_and_switching_it_off_restores_the_usual_signature() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "signature.txt", b"First file signature");
    let (mut app, account) = app();
    configure(&mut app, account, &path, true);
    assert!(compose_text(&mut app, account).contains("First file signature"));
    std::fs::write(&path, b"Second file signature").unwrap();
    assert!(app.session.compose.as_ref().unwrap().body.text().to_plain().contains("First file signature"));
    assert!(compose_text(&mut app, account).contains("Second file signature"));
    app.execute("ui.signatureFile", json!({"account": account, "enabled": false})).unwrap();
    std::fs::write(&path, [0xff]).unwrap();
    assert!(compose_text(&mut app, account).contains("Usual signature"));
    assert!(app.session.compose.as_ref().unwrap().signature_file_warning.is_empty());
    assert_eq!(setting(&app, account).path, path.to_str().unwrap());
    assert_eq!(app.session.mailbox.signature("Usual").unwrap().plain(), "Usual signature");
}

#[test]
fn missing_invalid_and_oversized_files_use_the_usual_signature_and_warn() {
    let dir = tempfile::tempdir().unwrap();
    let paths = [
        dir.path().join("gone.txt"),
        write(&dir, "unsupported.pdf", b"not a signature"),
        write(&dir, "invalid.txt", &[0xff, 0xfe]),
        write(&dir, "huge.txt", &vec![b'a'; MAX_PICTURE_BYTES + 1]),
        dir.path().to_path_buf(),
        write(&dir, "long.txt", &vec![b'a'; MAX_TEXT + 1]),
        write(&dir, "long.html", &vec![b'a'; MAX_HTML + 1]),
        write(&dir, "broken.png", b"\x89PNG\r\n\x1a\nbroken"),
    ];
    for path in paths {
        let (mut app, account) = app();
        configure(&mut app, account, &path, true);
        assert!(compose_text(&mut app, account).contains("Usual signature"), "{path:?}");
        assert!(!app.session.compose.as_ref().unwrap().signature_file_warning.is_empty(), "{path:?}");
    }
}

#[test]
fn html_is_kept_as_html_and_utf8_text_is_not_interpreted_as_markup() {
    let dir = tempfile::tempdir().unwrap();
    let text = write(&dir, "signature.txt", "\u{feff}Привет <b>text</b>".as_bytes());
    assert_eq!(load(&text).unwrap().plain(), "Привет <b>text</b>");
    let html = "<p><b>File signature</b></p>";
    let path = write(&dir, "signature.HTML", html.as_bytes());
    let (mut app, account) = app();
    configure(&mut app, account, &path, true);
    assert!(compose_text(&mut app, account).contains("File signature"));
    assert_eq!(app.session.compose.as_ref().unwrap().signature.as_ref().unwrap().html.as_deref(), Some(html));
}

#[test]
fn image_signature_is_an_inline_attachment_in_a_saved_message() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("signature.png");
    image::RgbaImage::from_pixel(4, 4, image::Rgba([30, 60, 90, 255])).save(&path).unwrap();
    let (mut app, account) = app();
    configure(&mut app, account, &path, true);
    compose_text(&mut app, account);
    assert_eq!(app.session.compose.as_ref().unwrap().signature.as_ref().unwrap().pictures.len(), 1);
    app.execute("compose.saveDraft", json!({})).unwrap();
    let draft = app.session.compose.as_ref().unwrap().draft.unwrap();
    let message = app.session.mailbox.message(draft).unwrap();
    assert!(message.body.html.as_ref().unwrap().contains("cid:"));
    assert!(message.attachments.iter().any(|a| a.mime == "image/png" && !a.data.is_empty()));
    let original = message.attachments.iter().find(|a| a.mime == "image/png").unwrap().data.clone();
    app.execute("compose.close", json!({})).unwrap();
    // Reopening uses the draft's snapshot even when its source no longer decodes.
    std::fs::write(&path, b"changed externally").unwrap();
    app.execute("mail.editDraft", json!({"id":draft})).unwrap();
    let compose = app.session.compose.as_ref().unwrap();
    assert_eq!(compose.pictures.len(), 1);
    assert_eq!(compose.body.text().pictures(), vec![compose.pictures[0].id]);
    assert!(compose.attachments.iter().all(|a| a.mime != "image/png"));
    app.execute("compose.saveDraft", json!({})).unwrap();
    let message = app.session.mailbox.message(draft).unwrap();
    assert!(message.body.html.as_ref().unwrap().contains("cid:pic-"));
    assert_eq!(message.attachments.iter().filter(|a| a.mime == "image/png").count(), 1);
    assert_eq!(message.attachments.iter().find(|a| a.mime == "image/png").unwrap().data, original);
}

#[test]
fn empty_file_means_no_signature_instead_of_an_empty_snapshot_or_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, account) = app();
    for text in ["", " \n\t"] {
        let path = write(&dir, "empty.txt", text.as_bytes());
        configure(&mut app, account, &path, true);
        assert!(!compose_text(&mut app, account).contains("Usual signature"));
        let c = app.session.compose.as_ref().unwrap();
        assert!(c.signature.is_none());
        assert!(c.signature_file_warning.is_empty());
    }
}

#[test]
fn reopened_signature_and_body_images_keep_distinct_ids_and_survive_another_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("signature.png");
    image::RgbaImage::from_pixel(4, 4, image::Rgba([90, 60, 30, 255])).save(&path).unwrap();
    let (mut app, account) = app();
    configure(&mut app, account, &path, true);
    compose_text(&mut app, account);
    app.execute("insert.picture", json!({"data":base64_lite::encode(&std::fs::read(&path).unwrap()), "file":"body.png", "at":0})).unwrap();
    app.execute("compose.saveDraft", json!({})).unwrap();
    let draft = app.session.compose.as_ref().unwrap().draft.unwrap();
    for _ in 0..2 {
        app.execute("compose.close", json!({})).unwrap();
        app.execute("mail.editDraft", json!({"id":draft})).unwrap();
        let c = app.session.compose.as_ref().unwrap();
        assert_eq!(c.pictures.len(), 2);
        assert_ne!(c.pictures[0].id, c.pictures[1].id);
        assert_eq!(c.body.text().pictures().len(), 2);
        app.execute("compose.saveDraft", json!({})).unwrap();
        assert_eq!(app.session.mailbox.message(draft).unwrap().attachments.iter().filter(|a| a.inline).count(), 2);
    }
}

#[test]
fn file_tab_and_choose_surface_command_failures_in_the_page() {
    use egui_kittest::kittest::Queryable as _;
    let mut w = crate::people_view::testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all":true})).unwrap();
    w.state_mut().execute("ui.backstage", json!({"value":true,"page":"signatures"})).unwrap();
    w.run_steps(3);
    for n in 0..200 {
        w.state_mut().ui.signature_files.insert(format!("unused-{n}"), Setting::default());
    }
    let file = w.state().hit.rect("signatureFileTab", 0).unwrap();
    crate::people_view::testing::click(&mut w, file.center(), egui::PointerButton::Primary, 1);
    w.run_steps(3);
    assert!(w.get_by_label("too many signature file settings").rect().is_positive());
    w.state_mut().ui.signature_files.clear();
    let file = w.state().hit.rect("signatureFileTab", 0).unwrap();
    crate::people_view::testing::click(&mut w, file.center(), egui::PointerButton::Primary, 1);
    w.run_steps(3);
    w.get_by_label(tl!("Choose…")).click();
    w.run_steps(3);
    assert!(w.state().signature_note.as_ref().is_some_and(|note| note.error));
    let error = w.state().signature_note.as_ref().unwrap().text.clone();
    assert!(w.get_by_label(&error).rect().is_positive());
}

#[test]
fn undo_and_redo_follow_the_exact_signature_operation_and_keep_no_old_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "signature.txt", b"File signature");
    let (mut app, account) = app();
    configure(&mut app, account, &path, true);
    app.execute("signature.rename", json!({"name":"Usual","to":"Renamed"})).unwrap();
    assert!(!setting_for_name(&app, "Usual").enabled);
    app.execute("edit.undo", json!({})).unwrap();
    assert!(setting_for_name(&app, "Usual").enabled);
    assert!(!setting_for_name(&app, "Renamed").enabled);
    app.execute("edit.redo", json!({})).unwrap();
    assert!(setting_for_name(&app, "Renamed").enabled);
    app.execute("signature.remove", json!({"name":"Renamed"})).unwrap();
    app.execute("edit.undo", json!({})).unwrap();
    assert!(setting_for_name(&app, "Renamed").enabled);
    app.execute("edit.redo", json!({})).unwrap();
    assert!(!setting_for_name(&app, "Renamed").enabled);
    // A new branch of history must not accidentally reuse the removed signature's source.
    app.execute("signature.set", json!({"name":"Renamed","text":"Replacement"})).unwrap();
    assert!(!setting_for_name(&app, "Renamed").enabled);
    app.execute("edit.undo", json!({})).unwrap();
    assert!(!setting_for_name(&app, "Renamed").enabled);
    app.execute("edit.undo", json!({})).unwrap();
    assert!(setting_for_name(&app, "Renamed").enabled);
    app.execute("edit.undo", json!({})).unwrap();
    assert!(setting_for_name(&app, "Usual").enabled);
    app.execute("signature.set", json!({"name":"Renamed","text":"Unrelated"})).unwrap();
    assert!(!setting_for_name(&app, "Renamed").enabled);
    assert_eq!(setting_for_name(&app, "Usual").path, path.to_str().unwrap());
}

#[test]
fn undo_rename_moves_a_file_chosen_after_the_rename_and_redo_keeps_later_edits() {
    let dir = tempfile::tempdir().unwrap();
    let first = write(&dir, "first.txt", b"First file");
    let second = write(&dir, "second.txt", b"Second file");
    let (mut app, account) = app();
    app.execute("signature.rename", json!({"name":"Usual","to":"Renamed"})).unwrap();
    configure(&mut app, account, &first, true);
    app.execute("edit.undo", json!({})).unwrap();
    assert_eq!(setting_for_name(&app, "Usual").path, first.to_str().unwrap());
    assert!(!setting_for_name(&app, "Renamed").enabled);
    configure(&mut app, account, &second, true);
    app.execute("edit.redo", json!({})).unwrap();
    assert_eq!(setting_for_name(&app, "Renamed").path, second.to_str().unwrap());
    assert!(!setting_for_name(&app, "Usual").enabled);
}

#[test]
fn signature_picker_is_cancelled_when_the_original_target_is_removed_or_renamed() {
    use std::sync::mpsc::channel;
    for command in ["signature.remove", "signature.rename"] {
        let (mut app, _) = app();
        let (_tx, rx) = channel();
        let rx = std::cell::RefCell::new(Some(rx));
        app.services.file_dialog = Some(Box::new(move |_| rx.borrow_mut().take().unwrap()));
        app.execute("ui.chooseSignatureFile", json!({"name":"Usual"})).unwrap();
        app.execute(command, json!({"name":"Usual","to":"Renamed"})).unwrap();
        app.execute("signature.set", json!({"name":"Usual","text":"Replacement"})).unwrap();
        assert!(app.file_dialog.is_none());
        assert!(!setting_for_name(&app, "Usual").enabled);
    }
}

#[test]
fn plain_reply_warns_and_offers_html_before_sending_the_image_signature() {
    use egui_kittest::kittest::Queryable as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("signature.png");
    image::RgbaImage::from_pixel(4, 4, image::Rgba([30, 60, 90, 255])).save(&path).unwrap();
    let mut w = crate::people_view::testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all":true})).unwrap();
    let account = w.state().session.mailbox.default_account().unwrap().id;
    configure(w.state_mut(), account, &path, true);
    let source = w.state().session.mailbox.messages.values().find(|m| m.account == account && !m.draft && m.body.html.is_none()).unwrap().id;
    w.state_mut().execute("mail.reply", json!({"id":source})).unwrap();
    w.run_steps(3);
    assert_eq!(w.state().session.compose.as_ref().unwrap().format, mailcraft_engine::BodyFormat::Plain);
    assert!(w.get_by_label(tl!("Pictures are not included in plain-text messages. Switch to HTML to include them.")).rect().is_positive());
    w.get_by_label(tl!("Switch to HTML")).click();
    w.run_steps(3);
    assert_eq!(w.state().session.compose.as_ref().unwrap().format, mailcraft_engine::BodyFormat::Html);
    w.state_mut().execute("compose.saveDraft", json!({})).unwrap();
    let id = w.state().session.compose.as_ref().unwrap().draft.unwrap();
    let message = w.state().session.mailbox.message(id).unwrap();
    assert!(message.body.html.as_ref().unwrap().contains("cid:sig-"));
    assert!(message.attachments.iter().any(|a| a.inline && a.mime == "image/png"));
    w.state_mut().execute("compose.formatPlain", json!({})).unwrap();
    w.state_mut().execute("compose.saveDraft", json!({})).unwrap();
    assert!(w.state().session.mailbox.message(id).unwrap().draft_plain);
}

#[test]
fn plain_draft_preserves_images_across_restart_and_sends_only_the_chosen_format() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("signature.png");
    image::RgbaImage::from_pixel(4, 4, image::Rgba([30, 60, 90, 255])).save(&path).unwrap();
    let (mut app, account) = app();
    configure(&mut app, account, &path, true);
    compose_text(&mut app, account);
    app.execute("compose.formatPlain", json!({})).unwrap();
    app.execute("compose.set", json!({"to":"recipient@example.org"})).unwrap();
    app.execute("compose.saveDraft", json!({})).unwrap();
    let draft = app.session.compose.as_ref().unwrap().draft.unwrap();
    let bytes = mailcraft_engine::io::write_mailbox(&app.session.mailbox).unwrap();
    // Neither reopening nor sending may reread the linked file.
    std::fs::write(&path, b"changed externally").unwrap();
    for html in [false, true] {
        let mailbox = mailcraft_engine::io::read_mailbox(&bytes).unwrap();
        let mut reopened = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
        reopened.autosave = false;
        reopened.session.driver.set_auto_sync(false);
        reopened.execute("mail.editDraft", json!({"id":draft})).unwrap();
        let c = reopened.session.compose.as_ref().unwrap();
        assert_eq!(c.format, mailcraft_engine::BodyFormat::Plain);
        assert!(c.has_pictures());
        assert_eq!(c.pictures.len(), 1);
        if html {
            reopened.execute("compose.formatHtml", json!({})).unwrap();
        }
        let id = reopened.execute("compose.send", json!({"confirmed":true})).unwrap()["id"].as_u64().unwrap();
        let sent = reopened.session.mailbox.message(id).unwrap();
        assert_eq!(sent.body.html.is_some(), html);
        assert_eq!(sent.attachments.iter().any(|a| a.inline && a.mime == "image/png"), html);
        assert!(!sent.draft_plain);
    }
}

#[test]
fn undo_creation_detaches_the_file_and_redo_restores_only_that_signature() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "extra.txt", b"Original file");
    let (mut app, _) = app();
    app.execute("signature.set", json!({"name":"Extra","text":"Saved"})).unwrap();
    app.execute("ui.signatureFile", json!({"name":"Extra","path":path,"enabled":true})).unwrap();
    app.execute("edit.undo", json!({})).unwrap();
    assert!(app.session.mailbox.signature("Extra").is_none());
    assert!(app.ui.signature_files.is_empty());
    app.execute("edit.redo", json!({})).unwrap();
    assert!(setting_for_name(&app, "Extra").enabled);
    assert_eq!(setting_for_name(&app, "Extra").path, path.to_str().unwrap());
    app.execute("edit.undo", json!({})).unwrap();
    app.execute("signature.set", json!({"name":"Extra","text":"Replacement"})).unwrap();
    assert!(!setting_for_name(&app, "Extra").enabled);
    assert!(app.ui.signature_files.is_empty());
}

#[test]
fn undo_reserves_file_slots_and_restored_preferences_keep_the_other_links() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "source.txt", b"Signature");
    let (mut app, account) = app();
    // A second signature already exists, so configuring it adds no mailbox history step.
    app.execute("signature.set", json!({"name":"Other","text":"Other"})).unwrap();
    configure(&mut app, account, &path, true);
    for n in 0..199 {
        app.ui.signature_files.insert(format!("other-mailbox-{n}"), Setting { enabled: true, path: path.to_str().unwrap().into() });
    }
    app.execute("signature.remove", json!({"name":"Usual"})).unwrap();
    assert_eq!(app.ui.signature_files.len(), 199);
    assert!(app.execute("ui.signatureFile", json!({"name":"Other","path":path,"enabled":true})).is_err());
    app.execute("edit.undo", json!({})).unwrap();
    assert_eq!(app.ui.signature_files.len(), 200);
    assert!(setting_for_name(&app, "Usual").enabled);
    let saved = app.prefs();
    app.apply_prefs(saved);
    assert_eq!(app.ui.signature_files.len(), 200);
    assert!(setting_for_name(&app, "Usual").enabled);
    // Oversized preferences from older builds retain valid entries instead of losing all.
    app.ui.signature_files.insert("zz-over-limit".into(), Setting::default());
    let saved = app.prefs();
    app.apply_prefs(saved);
    assert_eq!(app.ui.signature_files.len(), 200);
    assert!(setting_for_name(&app, "Usual").enabled);
}

#[test]
fn manual_insertion_reports_the_file_error_when_no_fallback_can_be_inserted() {
    let (mut app, account) = app();
    app.execute("signature.set", json!({"name":"Usual","text":""})).unwrap();
    app.execute("ui.signatureFile", json!({"name":"Usual","enabled":true,"path":"/nonexistent-signature.txt"})).unwrap();
    compose_text(&mut app, account);
    let before = app.session.compose.as_ref().unwrap().clone();
    let error = app.execute("compose.signature", json!({"name":"Usual"})).unwrap_err();
    assert!(error.contains("could not be read"), "{error}");
    assert_eq!(app.session.compose.as_ref().unwrap(), &before);
}

#[test]
fn picker_completion_displays_validation_failures_on_the_signature_page() {
    use std::sync::mpsc::channel;
    for stale in [false, true] {
        let (mut app, _) = app();
        let (tx, rx) = channel();
        let rx = std::cell::RefCell::new(Some(rx));
        app.services.file_dialog = Some(Box::new(move |_| rx.borrow_mut().take().unwrap()));
        app.execute("ui.chooseSignatureFile", json!({"name":"Usual"})).unwrap();
        if stale {
            app.session.path = Some("/another.mailcraft".into());
        } else {
            for n in 0..200 {
                app.ui.signature_files.insert(format!("other-{n}"), Setting::default());
            }
        }
        tx.send(Some(vec!["/signature.txt".into()])).unwrap();
        let error = files::poll(&mut app).unwrap().unwrap_err();
        let note = app.signature_note.as_ref().unwrap();
        assert!(note.error);
        assert_eq!(note.text, error);
    }
}

#[test]
fn deleted_pictures_retained_for_undo_do_not_show_the_plain_text_warning() {
    use egui_kittest::kittest::Queryable as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("picture.png");
    image::RgbaImage::from_pixel(4, 4, image::Rgba([30, 60, 90, 255])).save(&path).unwrap();
    let mut w = crate::people_view::testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all":true})).unwrap();
    w.state_mut().execute("mail.new", json!({})).unwrap();
    w.state_mut().execute("insert.picture", json!({"data":base64_lite::encode(&std::fs::read(path).unwrap()),"at":0})).unwrap();
    w.state_mut().execute("compose.select", json!({"from":0,"to":1})).unwrap();
    w.state_mut().execute("compose.delete", json!({})).unwrap();
    assert!(!w.state().session.compose.as_ref().unwrap().pictures.is_empty());
    assert!(!w.state().session.compose.as_ref().unwrap().has_pictures());
    w.state_mut().execute("compose.formatPlain", json!({})).unwrap();
    w.run_steps(3);
    assert!(w.query_by_label(tl!("Switch to HTML")).is_none());
}

#[test]
fn each_account_has_its_own_file_and_settings_are_local_not_in_the_mailbox() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "signature.txt", b"Only first account");
    let (mut app, account) = app();
    let other = app.execute("account.add", json!({"name":"Other", "email":"other@example.org"})).unwrap()["id"].as_u64().unwrap();
    configure(&mut app, account, &path, true);
    assert!(!compose_text(&mut app, other).contains("Only first account"));
    assert!(compose_text(&mut app, account).contains("Only first account"));
    let saved = serde_json::to_string(&app.prefs()).unwrap();
    assert!(!serde_json::to_string(&app.session.mailbox).unwrap().contains(path.to_str().unwrap()));
    let mailbox = app.session.mailbox.clone();
    let mut again = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    again.apply_prefs(serde_json::from_str(&saved).unwrap());
    assert!(compose_text(&mut again, account).contains("Only first account"));
    assert_eq!(serde_json::from_str::<UiState>("{}").unwrap().signature_files.len(), 0);
    again.session.path = Some(dir.path().join("another.mailcraft"));
    assert!(!compose_text(&mut again, account).contains("Only first account"));
}

#[test]
fn reply_and_forward_use_the_file_without_changing_reopened_drafts() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "signature.txt", b"Reply from file");
    let (mut app, account) = app();
    configure(&mut app, account, &path, true);
    let source = app.session.mailbox.messages.values().find(|m| m.account == account && !m.draft).unwrap().id;
    for command in ["mail.reply", "mail.replyAll", "mail.forward", "mail.forwardAsAttachment"] {
        app.execute(command, json!({"id": source})).unwrap();
        assert!(app.session.compose.as_ref().unwrap().body.text().to_plain().contains("Reply from file"), "{command}");
    }
    app.execute("compose.saveDraft", json!({})).unwrap();
    let draft = app.session.compose.as_ref().unwrap().draft.unwrap();
    app.execute("compose.close", json!({})).unwrap();
    std::fs::write(&path, b"Changed later").unwrap();
    app.execute("mail.editDraft", json!({"id": draft})).unwrap();
    assert!(app.session.compose.as_ref().unwrap().body.text().to_plain().contains("Reply from file"));
}

#[test]
fn switching_the_sending_account_replaces_an_unedited_file_signature() {
    let dir = tempfile::tempdir().unwrap();
    let first = write(&dir, "first.txt", b"First signature file");
    let second = write(&dir, "second.txt", b"Second signature file");
    let (mut app, account) = app();
    let other = app.execute("account.add", json!({"name":"Other", "email":"other@example.org"})).unwrap()["id"].as_u64().unwrap();
    configure(&mut app, account, &first, true);
    configure(&mut app, other, &second, true);
    assert!(compose_text(&mut app, account).contains("First signature file"));
    app.execute("compose.set", json!({"account":other})).unwrap();
    let text = app.session.compose.as_ref().unwrap().body.text().to_plain();
    assert!(text.contains("Second signature file"));
    assert!(!text.contains("First signature file"));
}

#[test]
fn existing_new_and_reply_selectors_can_assign_different_file_signatures() {
    let dir = tempfile::tempdir().unwrap();
    let first = write(&dir, "new.txt", b"New message file");
    let reply = write(&dir, "reply.txt", b"Reply file");
    let (mut app, account) = app();
    configure(&mut app, account, &first, true);
    app.execute("signature.set", json!({"name":"Reply","text":"Saved reply"})).unwrap();
    app.execute("signature.defaults", json!({"account":account,"reply":"Reply"})).unwrap();
    app.execute("ui.signatureFile", json!({"name":"Reply","enabled":true,"path":reply})).unwrap();
    assert!(compose_text(&mut app, account).contains("New message file"));
    let source = app.session.mailbox.messages.values().find(|m| m.account == account && !m.draft).unwrap().id;
    app.execute("mail.reply", json!({"id":source})).unwrap();
    let text = app.session.compose.as_ref().unwrap().body.text().to_plain();
    assert!(text.contains("Reply file"));
    assert!(!text.contains("New message file"));
}

#[test]
fn picker_cancellation_preserves_the_file_and_the_account_binding() {
    use std::sync::mpsc::channel;
    let (mut app, account) = app();
    let (tx, rx) = channel();
    let rx = std::cell::RefCell::new(Some(rx));
    app.services.file_dialog = Some(Box::new(move |request| {
        assert_eq!(request, files::FileDialogRequest::Open { purpose: "signatureFile".into() });
        rx.borrow_mut().take().unwrap()
    }));
    app.execute("ui.chooseSignatureFile", json!({"account": account})).unwrap();
    tx.send(None).unwrap();
    let _ = files::poll(&mut app);
    assert_eq!(setting(&app, account), Setting::default());
    assert_eq!(mailcraft_engine::agent::access("ui.signatureFile"), Some(mailcraft_engine::agent::Access::Never));
}

#[test]
fn picker_updates_only_the_account_that_opened_it_and_refuses_a_replaced_mailbox() {
    use std::sync::mpsc::channel;
    for replaced in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "signature.txt", b"Chosen file");
        let (mut app, account) = app();
        let (tx, rx) = channel();
        let rx = std::cell::RefCell::new(Some(rx));
        app.services.file_dialog = Some(Box::new(move |_| rx.borrow_mut().take().unwrap()));
        app.execute("ui.signatureFile", json!({"account":account, "enabled":true})).unwrap();
        app.execute("ui.chooseSignatureFile", json!({"account":account})).unwrap();
        if replaced {
            app.session.path = Some(dir.path().join("different.mailcraft"));
        }
        tx.send(Some(vec![path.to_str().unwrap().into()])).unwrap();
        let result = files::poll(&mut app).unwrap();
        assert_eq!(result.is_err(), replaced);
        if !replaced {
            assert_eq!(setting(&app, account).path, path.to_str().unwrap());
            assert!(compose_text(&mut app, account).contains("Chosen file"));
        } else {
            assert_eq!(setting(&app, account), Setting::default());
        }
    }
}

#[test]
fn agent_cannot_configure_a_file_even_with_settings_permission() {
    let (mut app, account) = app();
    app.session.agent = Some(vec![mailcraft_engine::agent::Class::Settings]);
    assert!(app.execute("ui.signatureFile", json!({"account":account,"path":"/private.txt","enabled":true})).is_err());
    assert!(app.execute("ui.chooseSignatureFile", json!({"account":account})).is_err());
    assert!(app.ui.signature_files.is_empty());
}

#[test]
fn malformed_configuration_is_rejected_without_changing_the_previous_setting() {
    let (mut app, account) = app();
    for params in [
        json!({"account":account,"enabled":"yes"}),
        json!({"account":account,"path":"relative.txt"}),
        json!({"account":account,"path":9}),
        json!({"account":account,"path":"/bad\u{0000}path"}),
        json!({"account":account,"path":format!("/{}", "a".repeat(4096))}),
        json!({"account":u64::MAX,"enabled":true}),
    ] {
        assert!(app.execute("ui.signatureFile", params).is_err());
        assert!(app.ui.signature_files.is_empty());
    }
}

#[test]
fn renaming_a_signature_keeps_its_file_and_removing_it_does_not_reuse_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "signature.txt", b"Named file source");
    let (mut app, account) = app();
    configure(&mut app, account, &path, true);
    app.execute("signature.rename", json!({"name":"Usual","to":"Renamed"})).unwrap();
    assert_eq!(setting_for_name(&app, "Renamed").path, path.to_str().unwrap());
    assert!(compose_text(&mut app, account).contains("Named file source"));
    app.execute("signature.remove", json!({"name":"Renamed"})).unwrap();
    app.execute("signature.set", json!({"name":"Renamed","text":"New saved content"})).unwrap();
    assert!(!setting_for_name(&app, "Renamed").enabled);
}

#[test]
fn preview_updates_immediately_after_choosing_a_file_and_refresh_reads_changes() {
    let dir = tempfile::tempdir().unwrap();
    let first = write(&dir, "one.txt", b"First preview");
    let second = write(&dir, "two.html", b"<b>Second preview</b>");
    let (mut app, account) = app();
    configure(&mut app, account, &first, true);
    assert!(preview(&mut app, "Usual").unwrap().unwrap().plain().contains("First preview"));
    configure(&mut app, account, &second, true);
    assert!(preview(&mut app, "Usual").unwrap().unwrap().plain().contains("Second preview"));
    std::fs::write(&second, b"<b>Changed preview</b>").unwrap();
    app.signature_file_preview = None;
    assert!(preview(&mut app, "Usual").unwrap().unwrap().plain().contains("Changed preview"));
    app.execute("ui.signatureFile", json!({"name":"Usual","enabled":false})).unwrap();
    assert!(preview(&mut app, "Usual").is_none());
}

#[test]
fn missing_file_with_no_usual_signature_still_opens_a_message_and_warns() {
    let (mut app, account) = app();
    app.execute("signature.set", json!({"name":"Usual","text":""})).unwrap();
    app.execute("ui.signatureFile", json!({"account":account,"enabled":true})).unwrap();
    compose_text(&mut app, account);
    let compose = app.session.compose.as_ref().unwrap();
    assert!(compose.signature.is_none());
    assert!(!compose.signature_file_warning.is_empty());
}

#[test]
fn file_tab_previews_the_file_and_preserves_the_original_account_selectors() {
    use egui::accesskit::Role;
    use egui_kittest::kittest::{NodeT as _, Queryable as _};
    let mut w = crate::people_view::testing::window("view.mail");
    let account = w.state().session.mailbox.default_account().unwrap().id;
    w.state_mut().execute("ui.dismissReminder", json!({"all":true})).unwrap();
    w.state_mut().execute("ui.backstage", json!({"value":true,"page":"signatures"})).unwrap();
    w.run_steps(4);
    let file = w.state().hit.rect("signatureFileTab", 0).unwrap();
    crate::people_view::testing::click(&mut w, file.center(), egui::PointerButton::Primary, 1);
    w.run_steps(3);
    assert!(w.query_by_role_and_label(Role::CheckBox, "Use a signature from a file (text, HTML, or image)").is_none());
    assert!(setting(w.state(), account).enabled);
    assert!(!w.get_by_role_and_label(Role::Button, tl!("Choose…")).accesskit_node().is_disabled());
    assert!(!w.get_by_label(tl!("Rich text")).accesskit_node().is_disabled());
    assert!(!w.get_by_label(tl!("HTML")).accesskit_node().is_disabled());
    for label in [tl!("New messages"), tl!("Replies and forwards")] {
        assert!(!w.get_by_role_and_label(Role::ComboBox, label).accesskit_node().is_disabled());
    }
    let dir = tempfile::tempdir().unwrap();
    let path = write(&dir, "preview.txt", b"Immediate file preview");
    configure(w.state_mut(), account, &path, true);
    w.run_steps(3);
    assert!(w.state().signature_file_preview.as_ref().unwrap().2.as_ref().unwrap().plain().contains("Immediate file preview"));
    assert!(w.get_all_by_label("Immediate file preview").any(|n| n.rect().is_positive()));
    w.get_by_label(tl!("Rich text")).click();
    w.run_steps(3);
    assert!(!setting(w.state(), account).enabled);
    assert!(w.query_by_role_and_label(Role::Button, tl!("Choose…")).is_none());
    assert!(!w.get_by_label(tl!("Rich text")).accesskit_node().is_disabled());
    for label in [tl!("New messages"), tl!("Replies and forwards")] {
        assert!(!w.get_by_role_and_label(Role::ComboBox, label).accesskit_node().is_disabled());
    }
    let file = w.state().hit.rect("signatureFileTab", 0).unwrap();
    crate::people_view::testing::click(&mut w, file.center(), egui::PointerButton::Primary, 1);
    w.run_steps(3);
    assert!(setting(w.state(), account).enabled);
    assert_eq!(setting(w.state(), account).path, path.to_str().unwrap());
    w.get_by_label(tl!("HTML")).click();
    w.run_steps(3);
    assert!(!setting(w.state(), account).enabled);
    assert!(w.state().session.signature_edit.as_ref().unwrap().signature.html().is_some());
}
