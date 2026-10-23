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
