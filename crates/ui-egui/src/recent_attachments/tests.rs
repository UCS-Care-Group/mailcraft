// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::cell::Cell;
use std::rc::Rc;

use egui::PointerButton;
use egui_kittest::kittest::Queryable;
use serde_json::json;

use crate::control::{ControlRequest, Outcome};
use crate::people_view::testing;

#[test]
fn opening_a_composer_does_not_load_recent_files_and_they_are_not_a_command() {
    let mut h = testing::window("view.mail");
    let calls = Rc::new(Cell::new(0));
    let count = Rc::clone(&calls);
    h.state_mut().services.recent_attachments = Some(Box::new(move |_| {
        count.set(count.get() + 1);
        std::sync::mpsc::channel().1
    }));
    h.state_mut().run("mail.new", json!({"subject": "Test message"})).unwrap();
    h.run_steps(5);
    assert_eq!(calls.get(), 0);
    let ctx = h.ctx.clone();
    let (req, _) = ControlRequest::new("engine.commands", json!({}));
    let Outcome::Done(reply) = crate::control::handle(h.state_mut(), &ctx, &req.from_agent(Vec::new())) else {
        panic!("expected command list");
    };
    assert!(!reply.to_string().contains("Recent files"));
    assert!(!reply.to_string().contains("recentAttachments"));
}

#[test]
fn recent_file_menu_is_hidden_from_captures_and_attaches_to_the_composer() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("mailcraft-recent.txt");
    let other_directory = directory.path().join("other");
    std::fs::create_dir(&other_directory).unwrap();
    let other = other_directory.join("mailcraft-recent.txt");
    std::fs::write(&file, b"recent file").unwrap();
    std::fs::write(&other, b"other recent file").unwrap();
    let name = file.file_name().unwrap().to_str().unwrap();
    let mut h = testing::window("view.mail");
    let calls = Rc::new(Cell::new(0));
    let count = Rc::clone(&calls);
    let recent = file.clone();
    h.state_mut().services.recent_attachments = Some(Box::new(move |_| {
        count.set(count.get() + 1);
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(vec![recent.clone(), other.clone()]).unwrap();
        rx
    }));
    let compose = h.state_mut().run("mail.new", json!({})).unwrap()["compose"].as_u64().unwrap();
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    h.run_steps(5);
    let arrow = h.query_all_by_label("Recent files").next().unwrap().rect().center();
    h.state_mut().headless_shots = true;
    testing::click(&mut h, arrow, PointerButton::Primary, 1);
    assert_eq!(calls.get(), 0);
    assert!(h.query_by_label(name).is_none());

    h.state_mut().headless_shots = false;
    h.state_mut().driven = true;
    testing::click(&mut h, arrow, PointerButton::Primary, 1);
    assert_eq!(calls.get(), 0);
    assert!(h.query_by_label(name).is_none());

    h.state_mut().driven = false;
    testing::click(&mut h, arrow, PointerButton::Primary, 1);
    let entries: Vec<_> = h.query_all_by_label(name).map(|node| node.rect().center()).collect();
    assert_eq!(entries.len(), 2);
    h.hover_at(entries[0]);
    h.run_steps(60);
    assert!(h.query_by_label(file.to_str().unwrap()).is_some());
    assert_eq!(calls.get(), 1);
    testing::click(&mut h, entries[0], PointerButton::Primary, 1);
    let message = h.state().session.composing_by_id(compose).unwrap();
    assert_eq!(message.attachments.len(), 1);
    assert_eq!(message.attachments[0].name, name);
    assert_eq!(message.attachments[0].data.as_slice(), b"recent file");
}
