// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Detail-pane edits must preserve steps retained by a concurrent merge.

use egui_kittest::kittest::Queryable as _;
use mailcraft_model::{MAX_TASK_STEPS, Mailbox, Task, TaskStep};
use serde_json::json;

#[test]
fn review_detail_checkbox_preserves_steps_outside_the_display_prefix() {
    let mut mailbox = Mailbox::with_local_account("Me", "me@example.org");
    let id = mailbox.add_task(Task {
        title: "Trip".into(),
        steps: (0..=MAX_TASK_STEPS).map(|i| TaskStep { id: format!("S{i}"), title: format!("Review step {i}"), completed: false }).collect(),
        ..Default::default()
    });
    let mut window = crate::people_view::testing::window_on(mailbox, "view.tasks");
    window.state_mut().execute("tasks.select", json!({"id":id})).unwrap();
    window.run_steps(2);
    window.get_by_label("Review step 0").click();
    window.run_steps(2);
    let task = window.state().session.mailbox.task(id).unwrap();
    assert!(task.steps[0].completed);
    assert_eq!(task.steps.len(), MAX_TASK_STEPS + 1);
    assert_eq!(task.steps.last().unwrap().id, format!("S{MAX_TASK_STEPS}"));
}
