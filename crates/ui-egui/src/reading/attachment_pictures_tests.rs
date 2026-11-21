// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Picture attachments shown as pictures (#834): which files count as pictures, the caps, the
//! decoding off the window's thread, the choice in File › Options and what it keeps.

use std::sync::Arc;

use egui::PointerButton;
use mailcraft_model::{Attachment, ContentRef};
use serde_json::json;

use super::attachment_pictures::{BOX, MAX_BROUGHT_IN, MAX_PREVIEWS, Thumbs, as_pictures, chosen, fit, may_be_picture, to_bring_in};
use super::images::MAX_PICTURE_BYTES;
use super::tests::{gif, jpeg, png, webp};
use crate::people_view::testing::{self, Window, click};
use crate::reading::attachment_key;

fn file(name: &str, mime: &str) -> Attachment {
    Attachment::new(name, mime, Vec::new())
}

/// An attachment of `len` bytes kept on disk, not in memory.
fn kept(name: &str, mime: &str, len: u64, tag: u8) -> Attachment {
    Attachment { kept: Some(ContentRef { sha256: [tag; 32], len }), ..file(name, mime) }
}

#[test]
fn what_may_be_a_picture_is_told_by_its_type_or_else_its_name() {
    for (name, mime) in [
        ("a.bin", "image/png"),
        ("a", "image/jpeg"),
        ("a", "IMAGE/JPEG; name=a.jpg"),
        ("a", "image/pjpeg"),
        ("a", "image/gif"),
        ("a", "image/webp"),
        ("IMG_20261108_101112.JPG", "application/octet-stream"),
        ("photo.jpeg", ""),
        ("scan.webp", "application/octet-stream"),
    ] {
        assert!(may_be_picture(&file(name, mime)), "{name} {mime}");
    }
    for (name, mime) in [
        ("a.png", "application/pdf"),
        ("a.png", "text/html"),
        ("drawing.svg", "image/svg+xml"),
        ("photo.heic", "image/heic"),
        ("a.svg", "application/octet-stream"),
        ("report.pdf", ""),
        ("png", "application/octet-stream"),
        ("archive.png.exe", "application/octet-stream"),
    ] {
        assert!(!may_be_picture(&file(name, mime)), "{name} {mime}");
    }
}

#[test]
fn what_is_brought_in_from_disk_is_capped() {
    let a = [
        kept("a.jpg", "image/jpeg", 1_000, 1),
        kept("b.pdf", "application/pdf", 1_000, 2),
        // In memory already.
        Attachment::new("c.png", "image/png", png(4, 4)),
        // Too large to be looked at.
        kept("d.png", "image/png", MAX_PICTURE_BYTES as u64 + 1, 4),
        kept("e.gif", "image/gif", 2_000, 5),
    ];
    let files: Vec<(usize, &Attachment)> = a.iter().enumerate().collect();
    let wanted = to_bring_in(&files);
    assert_eq!(wanted.iter().map(|k| k.sha256[0]).collect::<Vec<_>>(), [1, 5]);
    // At most so many pictures of a message …
    let many: Vec<Attachment> = (0..MAX_PREVIEWS + 10).map(|i| kept("p.png", "image/png", 10, i as u8)).collect();
    let files: Vec<(usize, &Attachment)> = many.iter().enumerate().collect();
    assert_eq!(to_bring_in(&files).len(), MAX_PREVIEWS);
    // … and so many bytes.
    let big = MAX_PICTURE_BYTES as u64;
    let heavy: Vec<Attachment> = (0..8).map(|i| kept("p.jpg", "image/jpeg", big, i)).collect();
    let files: Vec<(usize, &Attachment)> = heavy.iter().enumerate().collect();
    let wanted = to_bring_in(&files);
    assert_eq!(wanted.len() as u64, MAX_BROUGHT_IN / big);
    assert!(wanted.iter().map(|k| k.len).sum::<u64>() <= MAX_BROUGHT_IN);
    // What was brought in at an earlier frame counts: the cap holds from frame to frame. The
    // session leaves a file it brought in with its bytes and no `kept` (`Session::load_where`).
    let mut heavy = heavy;
    let brought = (MAX_BROUGHT_IN / big) as usize;
    for a in heavy.iter_mut().take(brought) {
        a.kept = None;
        a.data = Arc::new(vec![0; big as usize]);
    }
    let files: Vec<(usize, &Attachment)> = heavy.iter().enumerate().collect();
    assert!(to_bring_in(&files).is_empty());
    // One of them let go of: room for one more.
    let half: Vec<Attachment> = heavy.into_iter().skip(1).collect();
    let files: Vec<(usize, &Attachment)> = half.iter().enumerate().collect();
    assert_eq!(to_bring_in(&files).len(), 1);
}

#[test]
fn copies_of_a_chosen_picture_are_not_brought_in_with_it() {
    // Thirty attachments of one content (a mailbox file and a .pst store it once and name it
    // many times), some of them of no picture's type, and one other picture.
    let same = ContentRef { sha256: [9; 32], len: 20 * 1024 * 1024 };
    let mut a: Vec<Attachment> = (0..30)
        .map(|i| {
            let (name, mime) = if i % 5 == 4 { ("copy.bin", "application/pdf") } else { ("copy.jpg", "image/jpeg") };
            Attachment { kept: Some(same), ..file(name, mime) }
        })
        .collect();
    a.push(kept("other.png", "image/png", 1_000, 2));
    let files: Vec<(usize, &Attachment)> = a.iter().enumerate().collect();
    let wanted = to_bring_in(&files);
    // The cap holds the chosen ones to four of them (96 MB of 20 MB each): the first four.
    assert_eq!(wanted.len(), (MAX_BROUGHT_IN / same.len) as usize);
    // What the session is given picks those files, in order, and no more of them.
    let pick = chosen(&wanted);
    let picked: Vec<usize> = a.iter().enumerate().filter(|(_, a)| pick(a)).map(|(i, _)| i).collect();
    assert_eq!(picked, [0, 1, 2, 3]);
    // Thirty small copies: the chosen are the first 24 that may be a picture, no more of them,
    // and none that is of another type.
    let small = ContentRef { sha256: [8; 32], len: 1_000 };
    let b: Vec<Attachment> = (0..30)
        .map(|i| {
            let (name, mime) = if i % 5 == 4 { ("copy.bin", "application/pdf") } else { ("copy.jpg", "image/jpeg") };
            Attachment { kept: Some(small), ..file(name, mime) }
        })
        .collect();
    let files: Vec<(usize, &Attachment)> = b.iter().enumerate().collect();
    let wanted = to_bring_in(&files);
    assert_eq!(wanted.len(), MAX_PREVIEWS);
    let pick = chosen(&wanted);
    let picked: Vec<usize> = b.iter().enumerate().filter(|(_, a)| pick(a)).map(|(i, _)| i).collect();
    assert_eq!(picked.len(), MAX_PREVIEWS);
    assert!(picked.iter().all(|&i| i % 5 != 4), "{picked:?}");
    assert_eq!(picked, b.iter().enumerate().filter(|(_, a)| may_be_picture(a)).take(MAX_PREVIEWS).map(|(i, _)| i).collect::<Vec<_>>());
}

#[test]
fn only_so_many_pictures_of_a_message_are_shown_as_pictures() {
    let sizes: Vec<Option<[u32; 2]>> = (0..MAX_PREVIEWS + 5).map(|i| if i % 2 == 0 { Some([10, 10]) } else { None }).collect();
    let shown = as_pictures(&sizes);
    assert_eq!(shown.len(), sizes.len());
    assert_eq!(shown.iter().flatten().count(), (MAX_PREVIEWS + 5).div_ceil(2).min(MAX_PREVIEWS));
    let all: Vec<Option<[u32; 2]>> = vec![Some([1, 1]); MAX_PREVIEWS * 2];
    let shown = as_pictures(&all);
    assert_eq!(shown.iter().flatten().count(), MAX_PREVIEWS);
    assert!(shown.get(MAX_PREVIEWS).is_some_and(Option::is_none));
}

#[test]
fn a_picture_is_scaled_down_to_fit_and_never_up() {
    assert_eq!(fit([4000, 3000], BOX), egui::vec2(187.0, 140.0));
    assert_eq!(fit([3000, 400], BOX), egui::vec2(200.0, 27.0));
    assert_eq!(fit([32, 16], BOX), egui::vec2(32.0, 16.0));
    assert_eq!(fit([1, 16_000], BOX), egui::vec2(1.0, 140.0));
    assert_eq!(fit([0, 0], BOX), egui::vec2(1.0, 1.0));
}

/// The size `Thumbs` gives bytes, as a reading pane would ask.
fn size_of(bytes: Vec<u8>) -> Option<[u32; 2]> {
    let mut thumbs = Thumbs::default();
    thumbs.size(&Arc::new(bytes), 1)
}

#[test]
fn a_picture_is_told_by_its_bytes() {
    for (name, bytes) in [("png", png(64, 40)), ("jpeg", jpeg(64, 40)), ("gif", gif(64, 40)), ("webp", webp(64, 40))] {
        assert_eq!(size_of(bytes), Some([64, 40]), "{name}");
    }
    for (what, bytes) in [
        ("text", b"not a picture".to_vec()),
        ("svg", b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec()),
        ("html", b"<html><img src=https://example.org/x.png></html>".to_vec()),
        ("nothing", Vec::new()),
    ] {
        assert_eq!(size_of(bytes), None, "{what}");
    }
    let mut huge = png(8, 8);
    huge.resize(MAX_PICTURE_BYTES + 1, 0);
    assert_eq!(size_of(huge), None);
}

#[test]
fn pictures_are_decoded_off_the_windows_thread_one_at_a_time() {
    let ctx = egui::Context::default();
    let mut thumbs = Thumbs::default();
    thumbs.inline = false;
    let a = Arc::new(png(600, 300));
    let b = Arc::new(jpeg(100, 100));
    assert_eq!(thumbs.size(&a, 0), Some([600, 300]));
    assert_eq!(thumbs.size(&b, 0), Some([100, 100]));
    // Neither is there at once; the second waits for the first.
    assert!(thumbs.texture(&ctx, &a, [200, 200]).is_none());
    assert!(thumbs.texture(&ctx, &b, [100, 100]).is_none());
    let start = std::time::Instant::now();
    let texture = loop {
        thumbs.pump(&ctx);
        if let Some(texture) = thumbs.texture(&ctx, &a, [200, 200]) {
            break texture;
        }
        assert!(start.elapsed().as_secs() < 30, "the picture never came");
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert_eq!(texture.size(), [200, 100]);
    let start = std::time::Instant::now();
    while thumbs.texture(&ctx, &b, [100, 100]).is_none() {
        thumbs.pump(&ctx);
        assert!(start.elapsed().as_secs() < 30, "the second picture never came");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(thumbs.counts(), (2, 2));
    // What can't be decoded is not shown, and not tried again.
    let damaged = png(16, 16);
    let damaged = Arc::new(damaged.get(..damaged.len() / 2).unwrap().to_vec());
    assert_eq!(thumbs.size(&damaged, 1), Some([16, 16]));
    let start = std::time::Instant::now();
    while !thumbs.failed(&damaged) {
        thumbs.pump(&ctx);
        assert!(thumbs.texture(&ctx, &damaged, [16, 16]).is_none());
        assert!(start.elapsed().as_secs() < 30, "the damaged picture was never given up");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // A picture no message has any more is let go of.
    drop(a);
    assert_eq!(thumbs.size(&b, 2), Some([100, 100]));
    assert_eq!(thumbs.counts().0, 2);
}

/// Message 13 of the sample mailbox, in the reading pane, with a picture and a PDF.
const ID: u64 = 13;

fn window_with_picture() -> Window {
    let mut h = testing::window("view.mail");
    let app = h.state_mut();
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    app.session.mailbox.message_mut(ID).unwrap().attachments = vec![
        Attachment::new("IMG_20261108_101112.jpg", "image/jpeg", png(400, 300)),
        Attachment::new("notes.pdf", "application/pdf", b"%PDF".to_vec()),
    ];
    app.run("mail.select", json!({"id": ID})).unwrap();
    h.run_steps(5);
    h
}

fn item(h: &Window, index: usize) -> egui::Rect {
    h.state().hit.rect_in(0, "attachment", attachment_key(ID, index)).unwrap_or_else(|| panic!("no attachment {index}"))
}

#[test]
fn picture_attachments_are_shown_as_pictures_unless_the_owner_turns_it_off() {
    let mut h = window_with_picture();
    // Pictures at first: the picture is a picture, the PDF a chip, and no switch beside them.
    assert!(h.state().ui.attachment_pictures);
    assert!(item(&h, 0).height() > 100.0, "{:?}", item(&h, 0));
    assert!(item(&h, 1).height() < 40.0);
    let (kept, decoded) = Thumbs::of(&h.ctx).lock().unwrap().counts();
    assert!(kept >= 1 && decoded == 1, "{kept} {decoded}");
    assert!(h.state().hit.rect_in(0, "attachmentView", 0).is_none() && h.state().hit.rect_in(0, "attachmentView", 1).is_none());
    // Its `▾` is the chip's.
    assert!(h.state().hit.rect_in(0, "attachmentMenu", attachment_key(ID, 0)).is_some());
    // Names: chips, as classic Outlook shows them, for every message, and kept in ui.json.
    h.state_mut().execute("ui.attachmentPictures", json!({"value": false})).unwrap();
    h.run_steps(5);
    assert!(item(&h, 0).height() < 40.0 && item(&h, 1).height() < 40.0);
    let saved = serde_json::to_value(&h.state().ui).unwrap();
    assert_eq!(saved["attachmentPictures"], false);
    // And back.
    h.state_mut().execute("ui.attachmentPictures", json!({"value": true})).unwrap();
    h.run_steps(5);
    assert!(item(&h, 0).height() > 100.0);
}

#[test]
fn a_double_click_on_the_picture_opens_it() {
    let mut h = window_with_picture();
    let opened: Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let sink = opened.clone();
    h.state_mut().services.open_file = Some(Box::new(move |name, _| {
        sink.lock().unwrap().push(name.to_string());
        Ok(())
    }));
    h.state_mut().execute("ui.attachmentPictures", json!({"value": true})).unwrap();
    h.run_steps(5);
    let at = item(&h, 0).center();
    click(&mut h, at, PointerButton::Primary, 2);
    h.run_steps(3);
    assert_eq!(opened.lock().unwrap().as_slice(), ["IMG_20261108_101112.jpg"]);
}

#[test]
fn the_setting_is_kept_and_answers_the_command() {
    assert!(crate::UiState::default().attachment_pictures);
    // A ui.json from before keeps the default; one that says chips, chips.
    let read: crate::UiState = serde_json::from_value(json!({"theme": "dark"})).unwrap();
    assert!(read.attachment_pictures);
    let read: crate::UiState = serde_json::from_value(json!({"attachmentPictures": false})).unwrap();
    assert!(!read.attachment_pictures);
    let mut h = testing::window("view.mail");
    let app = h.state_mut();
    assert_eq!(app.execute("ui.attachmentPictures", json!({})).unwrap(), json!({"value": false}));
    assert_eq!(app.execute("ui.attachmentPictures", json!({"value": null})).unwrap(), json!({"value": true}));
    assert_eq!(app.execute("ui.attachmentPictures", json!({"value": false})).unwrap(), json!({"value": false}));
    assert_eq!(app.execute("ui.attachmentPictures", json!({"value": true})).unwrap(), json!({"value": true}));
    assert!(app.execute("ui.attachmentPictures", json!({"value": "on"})).is_err());
    assert!(app.ui.attachment_pictures);
}

#[test]
fn in_the_browser_one_picture_a_frame_and_the_next_frame_is_asked_for() {
    let ctx = egui::Context::default();
    let mut thumbs = Thumbs::default();
    thumbs.inline = true;
    let a = Arc::new(png(40, 20));
    let b = Arc::new(gif(20, 20));
    assert!(thumbs.texture(&ctx, &a, [40, 40]).is_some());
    // (A new context, and a texture made, ask for a frame of their own: start from none asked.)
    for _ in 0..3 {
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
    }
    let c = Arc::new(jpeg(20, 20));
    assert!(thumbs.texture(&ctx, &c, [20, 20]).is_some());
    assert!(!ctx.has_requested_repaint());
    // The second waits for the next frame, and the window is asked to draw one.
    assert!(thumbs.texture(&ctx, &b, [20, 20]).is_none());
    assert!(ctx.has_requested_repaint());
}

#[test]
fn the_content_compare_for_a_picture_read_again_is_bounded_in_a_frame() {
    let mut thumbs = Thumbs::default();
    // Many files of one length that differ only at the end: none is the same as another.
    let len = 512 * 1024;
    let all: Vec<Arc<Vec<u8>>> = (0..MAX_PREVIEWS)
        .map(|i| {
            let mut bytes = png(4, 4);
            bytes.resize(len, 0);
            bytes[len - 1] = i as u8;
            Arc::new(bytes)
        })
        .collect();
    for a in &all {
        thumbs.size(a, 1);
    }
    // The same files read anew: each is looked up against the others, within the bound.
    let anew: Vec<Arc<Vec<u8>>> = all.iter().map(|a| Arc::new(a.to_vec())).collect();
    for a in &anew {
        thumbs.size(a, 2);
    }
    assert!(thumbs.compared_in(2) <= super::attachment_pictures::MAX_COMPARED, "{}", thumbs.compared_in(2));
    assert!(thumbs.compared_in(2) > 0);
}

/// The picture's message moved into Junk E-mail (or a folder under it) before it is shown.
fn window_in_junk(in_subfolder: bool) -> Window {
    let mut h = testing::window("view.mail");
    let app = h.state_mut();
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    app.session.mailbox.message_mut(ID).unwrap().attachments = vec![
        Attachment::new("IMG_20261108_101112.jpg", "image/jpeg", png(400, 300)),
        Attachment::new("notes.pdf", "application/pdf", b"%PDF".to_vec()),
    ];
    let account = app.session.mailbox.message(ID).unwrap().account;
    let junk = app.session.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Junk).unwrap();
    let folder = if in_subfolder { app.session.mailbox.add_folder(account, Some(junk), "Held").unwrap() } else { junk };
    app.execute("mail.move", json!({"id": ID, "folder": folder})).unwrap();
    app.execute("folder.select", json!({"id": folder})).unwrap();
    app.execute("mail.select", json!({"id": ID})).unwrap();
    h.run_steps(5);
    h
}

#[test]
fn mail_in_junk_is_shown_by_names_only() {
    for in_subfolder in [false, true] {
        let h = window_in_junk(in_subfolder);
        // The picture is a chip, nothing was decoded or looked at.
        assert!(item(&h, 0).height() < 40.0 && item(&h, 1).height() < 40.0, "{in_subfolder}");
        assert_eq!(Thumbs::of(&h.ctx).lock().unwrap().counts(), (0, 0), "{in_subfolder}");
        // The owner's choice is as it was.
        assert!(h.state().ui.attachment_pictures);
    }
}

#[test]
fn a_message_with_very_many_attachments_costs_no_more_than_the_most_shown() {
    let mut h = testing::window("view.mail");
    let app = h.state_mut();
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    // A hundred pictures that differ at the end only, all of one length.
    app.session.mailbox.message_mut(ID).unwrap().attachments = (0..100)
        .map(|i| {
            let mut bytes = png(8, 8);
            bytes.resize(4096, 0);
            bytes[4095] = i as u8;
            Attachment::new(format!("p{i}.png"), "image/png", bytes)
        })
        .collect();
    app.run("mail.select", json!({"id": ID})).unwrap();
    h.run_steps(6);
    let (kept, _) = Thumbs::of(&h.ctx).lock().unwrap().counts();
    assert!(kept <= MAX_PREVIEWS, "{kept} pictures looked at");
    assert!(kept > 0);
}

/// The page of File › Options scrolled until the checkbox is in view, and its middle.
fn checkbox_in_view(h: &mut Window) -> egui::Pos2 {
    for _ in 0..6 {
        let rect = h.state().hit.rect("attachmentPicturesOption", 0).expect("the checkbox");
        let screen = h.ctx.content_rect();
        if screen.shrink(60.0).contains(rect.left_center()) {
            return rect.left_center() + egui::vec2(10.0, 0.0);
        }
        h.event(egui::Event::PointerMoved(screen.center()));
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -300.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
            source: egui::MouseWheelSource::Unknown,
        });
        h.run_steps(45);
    }
    panic!("the checkbox can't be scrolled into view");
}

#[test]
fn file_options_has_the_choice_as_a_checkbox() {
    let mut h = window_with_picture();
    h.state_mut().execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    h.run_steps(4);
    let at = checkbox_in_view(&mut h);
    assert!(h.state().ui.attachment_pictures);
    click(&mut h, at, PointerButton::Primary, 1);
    h.run_steps(3);
    assert!(!h.state().ui.attachment_pictures, "off");
    let at = checkbox_in_view(&mut h);
    click(&mut h, at, PointerButton::Primary, 1);
    h.run_steps(3);
    assert!(h.state().ui.attachment_pictures);
}
