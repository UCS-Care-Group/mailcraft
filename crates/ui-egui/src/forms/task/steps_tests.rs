// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Task steps stay in the editor's ordinary save/dirty command path.

use super::*;

#[test]
fn steps_are_loaded_edited_and_saved_without_changing_notes() {
    let mut s = Session::at(mailcraft_engine::sample::mailbox(1_790_000_000, 0), 1_790_000_000, 0);
    let made = s.run("tasks.new", &json!({"title": "Trip", "notes": "Keep this", "steps": [{"title": "Tea"}]})).unwrap();
    let id = made["id"].as_u64().unwrap();
    let mut form = TaskForm::edit(&s, &json!({"id": id})).unwrap();
    assert!(!form.dirty());
    form.steps[0].completed = true;
    form.steps.push(mailcraft_model::TaskStep { title: "Cake".into(), ..Default::default() });
    assert!(form.dirty());
    let (command, params) = form.command();
    assert!(params.get("notes").is_none());
    s.run(command, &params).unwrap();
    let task = s.mailbox.task(id).unwrap();
    assert_eq!(task.notes, "Keep this");
    assert_eq!(task.steps.len(), 2);
    assert!(task.steps[0].completed);
    form.steps.remove(0);
    let (command, params) = form.command();
    s.run(command, &params).unwrap();
    assert_eq!(s.mailbox.task(id).unwrap().steps.len(), 1);
}
