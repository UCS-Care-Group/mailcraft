// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The message list's group headings (#929): a click on a heading collapses its group or expands
//! it again, by the pointer or from the keyboard, Left and Right do the same, and a collapsed
//! heading says how many messages it stands for.

use egui::{Key, PointerButton, Rect, pos2};
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use mailcraft_model::Id;
use serde_json::json;

use crate::people_view::testing::{self, Window};

fn window() -> Window {
    let mut w = testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.state_mut().execute("folder.select", json!({"kind": "inbox"})).unwrap();
    w.run_steps(3);
    w
}

/// A group of the list with two messages or more: its heading and the row it heads.
fn a_group(w: &mut Window) -> (String, Id) {
    let rows = w.state_mut().session.rows();
    let mut best: Option<(String, Id, usize)> = None;
    for (i, r) in rows.iter().enumerate() {
        let Some(g) = &r.group else { continue };
        let size = rows[i..].iter().enumerate().take_while(|(j, x)| *j == 0 || x.group.is_none()).filter(|(_, x)| x.under.is_none()).count();
        if size >= 2 && best.is_none() {
            best = Some((g.clone(), r.id, size));
        }
    }
    let (group, id, _) = best.expect("a group of two messages or more");
    (group, id)
}

fn heading(w: &Window, id: Id) -> Rect {
    w.state().hit.rect("groupHeading", id).unwrap_or_else(|| panic!("no heading for {id}"))
}

fn collapsed(w: &Window, group: &str) -> bool {
    let s = &w.state().session;
    s.prefs.is_collapsed(s.view.folder, group)
}

/// A click on a heading's arrow collapses its group, which then says how many messages it has
/// and is collapsed for a screen reader; a double-click on the heading, as classic Outlook has
/// it, expands it again and counts once.
#[test]
fn the_arrow_collapses_and_a_double_click_expands() {
    let mut w = window();
    let (group, id) = a_group(&mut w);
    let r = heading(&w, id);
    testing::click(&mut w, pos2(r.min.x + 8.0, r.center().y), PointerButton::Primary, 1);
    assert!(collapsed(&w, &group));
    let n = w.state_mut().session.rows().iter().find(|r| r.folded.is_some()).and_then(|r| r.folded).unwrap();
    let label = format!("{group} ({n} messages)");
    let node = w.query_by_label(&label).unwrap_or_else(|| panic!("a heading labelled {label}"));
    assert_eq!(node.accesskit_node().data().is_expanded(), Some(false));

    let r = heading(&w, id);
    testing::click(&mut w, r.center(), PointerButton::Primary, 2);
    assert!(!collapsed(&w, &group));
    assert!(w.query_by_label(&label).is_none());
}

/// A click on a collapsed heading expands its group again, and selects none of its messages: the
/// heading is the switch for the group and stands for nothing else.
#[test]
fn a_click_on_a_collapsed_heading_expands_it() {
    let mut w = window();
    let (group, id) = a_group(&mut w);
    w.state_mut().execute("view.collapseGroup", json!({"group": group})).unwrap();
    w.run_steps(3);
    let members = w.state_mut().session.rows().iter().find(|r| r.folded.is_some()).unwrap().members.clone();
    let before = w.state().session.sel.messages.clone();
    let r = heading(&w, id);
    testing::click(&mut w, r.center(), PointerButton::Primary, 1);
    w.run_steps(3);
    assert!(!collapsed(&w, &group), "the group is open again");
    assert_eq!(w.state().session.sel.messages, before, "and the click selected nothing");
    assert!(members.len() > 1, "the group had more than one message to hide");
}

/// Left collapses the group the selected message is in, and Right on its heading expands it.
/// The message picked is not the group's first row: with that one the group's row keeps its id
/// whatever happens to the selection, and the test would hold without the rule it is here for.
#[test]
fn left_and_right_collapse_and_expand() {
    let mut w = window();
    let (group, head) = a_group(&mut w);
    let id = {
        let rows = w.state_mut().session.rows();
        let at = rows.iter().position(|r| r.id == head).expect("the group's first row");
        rows.get(at + 1).filter(|r| r.group.is_none()).map(|r| r.id).expect("a second row in the group")
    };
    w.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    w.run_steps(3);
    testing::key(&mut w, Key::ArrowLeft);
    assert!(collapsed(&w, &group));
    // The message picked is one the fold now hides, so it leaves the selection and the group's
    // own row takes its place: that row is what Right expands from.
    assert_eq!(w.state().session.sel.messages, vec![head], "the group's row is selected, not the message it hides");
    assert_ne!(head, id);
    testing::key(&mut w, Key::ArrowRight);
    assert!(!collapsed(&w, &group));
}

/// The pictures for the pull request, in both themes, when `MAILCRAFT_SHOTS_DIR` names a folder:
/// a group collapsed, and every group collapsed with Older Mail by Month and Year, for an Inbox
/// whose oldest messages are moved back by months and a year.
#[test]
fn shots() {
    let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") else { return };
    let dir = std::path::Path::new(&dir);
    let save = |w: &mut Window, name: &str| {
        for theme in ["light", "dark"] {
            w.state_mut().execute("ui.theme", json!({"value": theme})).unwrap();
            w.run_steps(4);
            w.render().unwrap().save(dir.join(format!("{name}-{theme}.png"))).unwrap();
        }
    };
    let mut w = window();
    let (group, _) = a_group(&mut w);
    w.state_mut().execute("view.collapseGroup", json!({"group": group})).unwrap();
    save(&mut w, "groups");

    let mut mb = mailcraft_engine::sample::mailbox(testing::now(), 0);
    let inbox = mb.inbox().unwrap();
    let mut oldest: Vec<(i64, Id)> =
        mb.messages.values().filter(|m| m.folder == inbox && !m.pinned).map(|m| (mailcraft_engine::list::when(m), m.id)).collect();
    oldest.sort();
    let back = [(2026, 8, 3), (2026, 6, 15), (2026, 2, 10), (2025, 11, 20)];
    for ((_, id), (y, mo, d)) in oldest.iter().zip(back) {
        let at = mailcraft_time::DateTime::new(mailcraft_time::Date::new(y, mo, d).unwrap(), 9, 30, 0).unwrap().to_timestamp(0);
        let m = std::sync::Arc::make_mut(mb.messages.get_mut(id).unwrap());
        (m.date, m.received) = (at, at);
    }
    let mut w = testing::window_on(mb, "view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.state_mut().execute("folder.select", json!({"kind": "inbox"})).unwrap();
    w.state_mut().execute("view.olderByMonth", json!({"value": true})).unwrap();
    w.state_mut().execute("view.collapseAllGroups", json!({})).unwrap();
    save(&mut w, "groups-older");
}

/// A heading activated without the pointer (from the keyboard, or by a screen reader) collapses
/// its group, and activated again expands it.
#[test]
fn a_heading_activated_without_the_pointer_toggles() {
    let mut w = window();
    let (group, _) = a_group(&mut w);
    let node = w.query_all(egui_kittest::kittest::By::new().role(egui::accesskit::Role::Button).label(&group)).next().expect("the group's heading");
    node.click();
    // (Well past the double-click time, so that the next one counts as a click of its own.)
    w.run_steps(40);
    assert!(collapsed(&w, &group));
    let n = w.state_mut().session.rows().iter().find_map(|r| r.folded.map(|_| r.members.len())).unwrap();
    let label = format!("{group} ({n} messages)");
    let node = w.query_all(egui_kittest::kittest::By::new().role(egui::accesskit::Role::Button).label(&label)).next().unwrap();
    node.click();
    w.run_steps(4);
    assert!(!collapsed(&w, &group));
}

/// A heading is a switch only where the groups collapse (#929). With the groups turned off there
/// are none; under categories a message is listed under each of its own, and during a search
/// every group shows whole. A click there would send a command the engine refuses, so the
/// heading is drawn and left alone. Pinned and "No category" carry no category of their own and
/// would otherwise still look clickable under categories.
#[test]
fn a_heading_is_no_switch_where_the_groups_do_not_collapse() {
    let mut w = window();
    let (group, _) = a_group(&mut w);
    let switch =
        |w: &Window, name: &str| w.query_all(egui_kittest::kittest::By::new().role(egui::accesskit::Role::Button).label(name)).next().is_some();
    assert!(switch(&w, &group), "a switch to begin with");

    // Arranged by categories, including the headings that are no category: Pinned, No category.
    w.state_mut().execute("view.sort", json!({"by": "categories"})).unwrap();
    w.run_steps(3);
    let headings: Vec<String> = w.state_mut().session.rows().iter().filter_map(|r| r.group.clone()).collect();
    assert!(headings.iter().any(|h| h == "Pinned"), "the sample's Inbox has a pinned message: {headings:?}");
    for h in &headings {
        assert!(!switch(&w, h), "{h} is no switch under categories");
    }

    // While something is searched for, every group shows whole.
    w.state_mut().execute("view.sort", json!({"by": "date"})).unwrap();
    w.state_mut().execute("view.search", json!({"query": "is:unread"})).unwrap();
    w.run_steps(3);
    let searched: Vec<String> = w.state_mut().session.rows().iter().filter_map(|r| r.group.clone()).collect();
    for h in &searched {
        assert!(!switch(&w, h), "{h} is no switch during a search");
    }

    // And a switch again once the search ends.
    w.state_mut().execute("view.clearSearch", json!({})).unwrap();
    w.run_steps(3);
    assert!(switch(&w, &group), "a switch again");
}
