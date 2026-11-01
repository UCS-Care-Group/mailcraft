// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tasks arranged by categories (#327).

use serde_json::json;

use super::*;

/// Arranged by categories, as in Outlook: a task is listed under each of its categories, those
/// with none first, by due date under each, and counts once.
#[test]
fn arranged_by_categories_a_task_is_under_each() {
    use crate::people_view::testing;
    let mut w = testing::window("view.tasks");
    let ctx = w.state().ctx.clone().unwrap();
    w.state_mut().execute("view.taskFilter", json!({"value": "all"})).unwrap();
    // The sample's tasks, found by title: overdue, today, the day after tomorrow, no date.
    let titled = |w: &egui_kittest::Harness<'_, MailApp>, title: &str| {
        w.state().session.mailbox.tasks.values().find(|t| t.title == title).map(|t| t.id).unwrap_or_else(|| panic!("no task {title:?} in the sample"))
    };
    let [overdue, today, later, undated] = [
        "Submit the September expense report",
        "Review the Q4 roadmap draft",
        "Write the 4.2 release notes",
        "Read up on Lisbon conference sessions",
    ]
    .map(|t| titled(&w, t));
    let ids: Vec<Id> = w.state().session.mailbox.tasks.keys().copied().collect();
    for id in &ids {
        w.state_mut().session.mailbox.task_mut(*id).unwrap().categories.clear();
    }
    for (id, names) in [
        (later, vec!["Red category", "blue category"]),
        (overdue, vec!["Red category"]),
        (today, vec!["Blue category"]),
        (undated, vec!["Red category"]),
    ] {
        w.state_mut().session.mailbox.task_mut(id).unwrap().categories = names.into_iter().map(String::from).collect();
    }
    w.state_mut().execute("tasks.arrange", json!({"by": "categories"})).unwrap();
    let l = lists(w.state(), &ctx);
    let mut heading = String::new();
    let mut under: Vec<(String, Id)> = Vec::new();
    for r in &l.layout.rows {
        if let Some(h) = r.heading {
            heading = match h {
                Heading::Category(i) => l.names.get(i as usize).cloned().unwrap_or_default(),
                Heading::NoCategory => "No category".into(),
                Heading::Due(b) => b.label().into(),
            };
        }
        under.push((heading.clone(), r.id));
    }
    let of = |name: &str| under.iter().filter(|(h, _)| h == name).map(|(_, id)| *id).collect::<Vec<_>>();
    // By due date under each heading, the undated last; "blue category" is Blue category.
    assert_eq!(of("Blue category"), [today, later]);
    assert_eq!(of("Red category"), [overdue, later, undated]);
    let none = of("No category");
    assert!(!none.is_empty() && none.iter().all(|id| ![overdue, today, later, undated].contains(id)));
    let mut headings: Vec<&str> = under.iter().map(|(h, _)| h.as_str()).collect();
    headings.dedup();
    assert_eq!(headings, ["No category", "Blue category", "Red category"]);
    assert_eq!(listed(&l.layout), ids.len(), "a task listed twice counts once");
    w.run_steps(3);
    w.state_mut().execute("tasks.arrange", json!({"by": "dueDate"})).unwrap();
    assert_eq!(listed(&lists(w.state(), &ctx).layout), lists(w.state(), &ctx).layout.rows.len());
    assert!(w.state_mut().execute("tasks.arrange", json!({"by": "colour"})).is_err());
}

/// The selected task keeps its highlight when the row that had it goes: its categories change, or
/// the list is arranged by due date again.
#[test]
fn the_selection_stays_on_a_row_when_its_row_goes() {
    use crate::people_view::testing;
    let mut w = testing::window("view.tasks");
    let ctx = w.state().ctx.clone().unwrap();
    w.state_mut().execute("view.taskFilter", json!({"value": "all"})).unwrap();
    let id = lists(w.state(), &ctx).layout.rows[0].id;
    w.state_mut().session.mailbox.task_mut(id).unwrap().categories = vec!["Blue category".into(), "Red category".into()];
    w.state_mut().execute("tasks.arrange", json!({"by": "categories"})).unwrap();
    w.state_mut().execute("tasks.select", json!({"id": id})).unwrap();
    choose_copy(&ctx, TASK_ROW, id, 1);
    w.run_steps(2);
    let highlighted = |w: &egui_kittest::Harness<'_, MailApp>| {
        let l = lists(w.state(), &ctx);
        l.layout
            .rows
            .iter()
            .filter(|r| row_selected(&ctx, TASK_ROW, w.state().session.sel.task, (r.id, r.copy)))
            .map(|r| (r.id, r.copy))
            .collect::<Vec<_>>()
    };
    assert_eq!(highlighted(&w), [(id, 1)]);

    // As the owner does it, so the list sees the change.
    w.state_mut().execute("tasks.categorize", json!({"category": "Blue category", "value": false, "id": id})).unwrap();
    w.run_steps(2);
    assert_eq!(highlighted(&w), [(id, 0)]);

    w.state_mut().execute("tasks.categorize", json!({"category": "Blue category", "value": true, "id": id})).unwrap();
    w.run_steps(2);
    choose_copy(&ctx, TASK_ROW, id, 1);
    w.state_mut().execute("tasks.arrange", json!({"by": "dueDate"})).unwrap();
    w.run_steps(2);
    assert_eq!(highlighted(&w), [(id, 0)]);
}
