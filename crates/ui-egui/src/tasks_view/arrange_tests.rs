// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tasks arranged by categories (#327).

use serde_json::json;

use super::*;

/// Arranged by categories, as in Outlook: a task is listed under each of its categories, those
/// with none first, and counts once.
#[test]
fn arranged_by_categories_a_task_is_under_each() {
    use crate::people_view::testing;
    let mut w = testing::window("view.tasks");
    let ctx = w.state().ctx.clone().unwrap();
    w.state_mut().execute("view.taskFilter", json!({"value": "all"})).unwrap();
    let ids: Vec<Id> = lists(w.state(), &ctx).layout.rows.iter().map(|r| r.id).collect();
    assert!(ids.len() >= 2);
    for id in &ids {
        w.state_mut().session.mailbox.task_mut(*id).unwrap().categories.clear();
    }
    for (id, names) in ids.iter().zip([vec!["Red category", "blue category"], vec!["Blue category"]]) {
        w.state_mut().session.mailbox.task_mut(*id).unwrap().categories = names.into_iter().map(String::from).collect();
    }
    w.state_mut().execute("tasks.arrange", json!({"by": "categories"})).unwrap();
    let l = lists(w.state(), &ctx);
    let label = |h: Heading| match h {
        Heading::Category(i) => l.names.get(i as usize).cloned().unwrap_or_default(),
        Heading::NoCategory => "No category".into(),
        Heading::Due(b) => b.label().into(),
    };
    let headings: Vec<String> = l.layout.rows.iter().filter_map(|r| r.heading).map(label).collect();
    let expected: &[&str] = if ids.len() > 2 { &["No category", "Blue category", "Red category"] } else { &["Blue category", "Red category"] };
    assert_eq!(headings, expected);
    assert_eq!(l.layout.rows.iter().filter(|r| r.id == ids[0]).count(), 2);
    assert_eq!(listed(&l.layout), ids.len());
    w.run_steps(3);
    w.state_mut().execute("tasks.arrange", json!({"by": "dueDate"})).unwrap();
    assert_eq!(listed(&lists(w.state(), &ctx).layout), lists(w.state(), &ctx).layout.rows.len());
    assert!(w.state_mut().execute("tasks.arrange", json!({"by": "colour"})).is_err());
}
