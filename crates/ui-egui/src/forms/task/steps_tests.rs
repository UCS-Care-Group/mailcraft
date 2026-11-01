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
    let mut form = TaskForm::edit(&s, &json!({"id": id})).unwrap();
    form.steps.remove(0);
    let (command, params) = form.command();
    s.run(command, &params).unwrap();
    assert_eq!(s.mailbox.task(id).unwrap().steps.len(), 1);
}

#[test]
fn review_editing_a_merged_checklist_keeps_the_hidden_tail() {
    let mut s = Session::at(mailcraft_engine::sample::mailbox(1_790_000_000, 0), 1_790_000_000, 0);
    let id = s.mailbox.add_task(mailcraft_model::Task {
        title: "Trip".into(),
        steps: (0..=mailcraft_model::MAX_TASK_STEPS)
            .map(|i| mailcraft_model::TaskStep { id: format!("S{i}"), title: format!("Step {i}"), completed: false })
            .collect(),
        ..Default::default()
    });
    let mut form = TaskForm::edit(&s, &json!({"id":id})).unwrap();
    assert_eq!(form.steps.len(), mailcraft_model::MAX_TASK_STEPS + 1);
    form.steps[0].completed = true;
    let (command, params) = form.command();
    s.run(command, &params).unwrap();
    assert_eq!(s.mailbox.task(id).unwrap().steps.len(), mailcraft_model::MAX_TASK_STEPS + 1);
}

#[test]
fn review_a_file_above_the_merge_bound_never_saves_a_truncated_prefix() {
    let mut s = Session::at(mailcraft_engine::sample::mailbox(1_790_000_000, 0), 1_790_000_000, 0);
    let id = s.mailbox.add_task(mailcraft_model::Task {
        title: "Trip".into(),
        steps: (0..=mailcraft_model::MAX_MERGED_TASK_STEPS)
            .map(|i| mailcraft_model::TaskStep { id: format!("S{i}"), title: format!("Step {i}"), completed: false })
            .collect(),
        ..Default::default()
    });
    let mut form = TaskForm::edit(&s, &json!({"id":id})).unwrap();
    form.title = "Changed title".into();
    let (command, params) = form.command();
    assert!(params.get("steps").is_none());
    s.run(command, &params).unwrap();
    assert_eq!(s.mailbox.task(id).unwrap().steps.len(), mailcraft_model::MAX_MERGED_TASK_STEPS + 1);
}

#[test]
fn review_a_stale_form_cannot_remove_newly_downloaded_steps() {
    let mut s = Session::at(mailcraft_engine::sample::mailbox(1_790_000_000, 0), 1_790_000_000, 0);
    let id = s.mailbox.add_task(mailcraft_model::Task {
        title: "Trip".into(),
        steps: vec![mailcraft_model::TaskStep { id: "S1".into(), title: "Tea".into(), completed: false }],
        ..Default::default()
    });
    let mut form = TaskForm::edit(&s, &json!({"id":id})).unwrap();
    let mut metadata = form.clone();
    s.mailbox.task_mut(id).unwrap().steps.push(mailcraft_model::TaskStep {
        id: "S2".into(),
        title: "Downloaded while editing".into(),
        completed: false,
    });
    form.steps[0].completed = true;
    form.title = "Stale title".into();
    let (command, params) = form.command();
    assert!(s.run(command, &params).unwrap_err().to_string().contains("Reopen"));
    let task = s.mailbox.task(id).unwrap();
    assert_eq!(task.title, "Trip");
    assert_eq!(task.steps.len(), 2);
    assert!(!task.steps[0].completed);
    metadata.title = "Metadata only".into();
    let (command, params) = metadata.command();
    s.run(command, &params).unwrap();
    assert_eq!(s.mailbox.task(id).unwrap().steps.len(), 2);
    let mut fresh = TaskForm::edit(&s, &json!({"id":id})).unwrap();
    fresh.steps[0].completed = true;
    let (command, params) = fresh.command();
    s.run(command, &params).unwrap();
    assert!(s.mailbox.task(id).unwrap().steps[0].completed);
    assert_eq!(s.mailbox.task(id).unwrap().steps.len(), 2);
}
