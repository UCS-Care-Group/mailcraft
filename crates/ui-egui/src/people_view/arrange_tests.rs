// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! People arranged by categories (#327).

use serde_json::json;

use super::*;

/// Arranged by categories, as in Outlook: a contact is listed under each of its categories,
/// those with none first, and counts once.
#[test]
fn the_list_arranged_by_categories_is_headed_by_them() {
    let mut w = testing::window("view.people");
    let ctx = w.state().ctx.clone().unwrap();
    let ids: Vec<Id> = list_layout(w.state(), &ctx).rows.iter().map(|r| r.id).collect();
    for id in &ids {
        w.state_mut().session.mailbox.contact_mut(*id).unwrap().categories.clear();
    }
    for (id, names) in ids.iter().zip([vec!["Red category", "Blue category"], vec!["Green category"]]) {
        w.state_mut().session.mailbox.contact_mut(*id).unwrap().categories = names.into_iter().map(String::from).collect();
    }
    w.state_mut().execute("people.arrange", json!({"by": "categories"})).unwrap();
    let list = list_layout(w.state(), &ctx);
    let headings: Vec<String> = list.rows.iter().filter_map(|r| r.heading).map(|g| group_label(g, &list.names)).collect();
    // As Outlook lists them: a contact under each of its categories.
    assert_eq!(headings, ["No category", "Blue category", "Green category", "Red category"]);
    assert_eq!(list.rows.iter().filter(|r| r.id == ids[0]).map(|r| r.copy).collect::<Vec<_>>(), [0, 1]);
    assert_eq!(listed(&list.layout), ids.len(), "a contact listed twice counts once");
    // Down from the contact's first row reaches its second, which is then the one selected.
    let first = list.rows.iter().position(|r| r.id == ids[0]).unwrap();
    let second = list.rows.iter().position(|r| r.id == ids[0] && r.copy == 1).unwrap();
    assert_eq!(list.neighbour_row(Some((ids[0], 0)), (second - first) as i64), Some((ids[0], 1)));
    assert_eq!(list.neighbour_row(Some((ids[0], 1)), 0), Some((ids[0], 1)));
    // It draws.
    w.run_steps(3);
}

/// The contacts under each category run by name, whatever other categories they have.
#[test]
fn under_each_category_contacts_are_by_name() {
    let mut w = testing::window("view.people");
    let ctx = w.state().ctx.clone().unwrap();
    let ids: Vec<Id> = list_layout(w.state(), &ctx).rows.iter().map(|r| r.id).collect();
    for id in &ids {
        w.state_mut().session.mailbox.contact_mut(*id).unwrap().categories.clear();
    }
    for (id, (first, last, names)) in
        ids.iter().zip([("Wen", "Zhao", vec!["Blue category", "Red category"]), ("Ada", "Brooks", vec!["Red category"])])
    {
        let c = w.state_mut().session.mailbox.contact_mut(*id).unwrap();
        (c.first, c.last) = (first.into(), last.into());
        c.categories = names.into_iter().map(String::from).collect();
    }
    w.state_mut().execute("people.arrange", json!({"by": "categories"})).unwrap();
    let list = list_layout(w.state(), &ctx);
    let mut heading = String::new();
    let mut red = Vec::new();
    for r in &list.rows {
        if let Some(g) = r.heading {
            heading = group_label(g, &list.names);
        }
        if heading == "Red category" {
            red.push(r.id);
        }
    }
    assert_eq!(red, [ids[1], ids[0]], "Brooks before Zhao");
}

/// The selected contact keeps its highlight when the row that had it goes: its categories change,
/// or the list is arranged by name again.
#[test]
fn the_selection_stays_on_a_row_when_its_row_goes() {
    let mut w = testing::window("view.people");
    let ctx = w.state().ctx.clone().unwrap();
    let id = list_layout(w.state(), &ctx).rows[0].id;
    w.state_mut().session.mailbox.contact_mut(id).unwrap().categories = vec!["Blue category".into(), "Red category".into()];
    w.state_mut().execute("people.arrange", json!({"by": "categories"})).unwrap();
    w.state_mut().execute("people.select", json!({"id": id})).unwrap();
    choose_copy(&ctx, PEOPLE_ROW, id, 1);
    w.run_steps(2);
    let highlighted = |w: &egui_kittest::Harness<'_, MailApp>| {
        let list = list_layout(w.state(), &ctx);
        list.rows
            .iter()
            .filter(|r| row_selected(&ctx, PEOPLE_ROW, w.state().session.sel.contact, (r.id, r.copy)))
            .map(|r| (r.id, r.copy))
            .collect::<Vec<_>>()
    };
    assert_eq!(highlighted(&w), [(id, 1)]);

    // Its second category taken away: its one row has the highlight.
    // As the owner does it, so the list sees the change.
    w.state_mut().execute("people.categorize", json!({"category": "Blue category", "value": false, "id": id})).unwrap();
    w.run_steps(2);
    assert_eq!(highlighted(&w), [(id, 0)]);

    // Arranged by name again, from its second row.
    w.state_mut().execute("people.categorize", json!({"category": "Blue category", "value": true, "id": id})).unwrap();
    w.run_steps(2);
    choose_copy(&ctx, PEOPLE_ROW, id, 1);
    w.state_mut().execute("people.arrange", json!({"by": "name"})).unwrap();
    w.run_steps(2);
    assert_eq!(highlighted(&w), [(id, 0)]);
}

/// A row of the same item under another category is a new selection to scroll to: Down from a
/// contact's last row under one category can reach its row under the next.
#[test]
fn another_row_of_the_same_item_is_scrolled_to() {
    // (the row selected each frame, what the list made of it)
    type Frames = (Vec<Option<(Id, u16)>>, Vec<bool>);
    let rows = vec![Some((7, 0)), Some((7, 0)), Some((7, 1)), Some((7, 1)), None];
    let mut w = egui_kittest::Harness::builder().build_ui_state(
        |ui, (rows, seen): &mut Frames| {
            if let Some(row) = rows.get(seen.len()).copied() {
                seen.push(selection_changed(ui, "test_list_seen", row));
            }
        },
        (rows, Vec::new()),
    );
    w.run_steps(5);
    assert_eq!(w.state().1, [true, false, true, false, true]);
}
