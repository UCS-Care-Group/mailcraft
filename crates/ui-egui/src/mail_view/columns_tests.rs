// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #344: real header clicks, saved configuration and aligned cells.
use super::*;
use crate::people_view::testing;
use egui::PointerButton;
use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::SortBy;

#[test]
fn headers_sort_in_both_directions_and_selection_survives() {
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all":true})).unwrap();
    w.state_mut().execute("view.readingPane", json!({"value":"off"})).unwrap();
    w.state_mut().execute("view.columns", json!({"enabled":true})).unwrap();
    w.run_steps(3);
    let id = w.state_mut().session.rows().first().unwrap().id;
    w.state_mut().execute("mail.select", json!({"id":id})).unwrap();
    w.run_steps(2);
    let selected = w.state().session.sel.clone();
    let center = w.state().hit.rect("columnHeader", Column::Subject as u64).unwrap().center();
    testing::click(&mut w, center, PointerButton::Primary, 1);
    assert_eq!(w.state().session.view.sort, SortBy::Subject);
    assert!(!w.state().session.view.sort_descending);
    testing::click(&mut w, center, PointerButton::Primary, 1);
    assert!(w.state().session.view.sort_descending);
    assert_eq!(w.state().session.sel, selected);
}

#[test]
fn settings_round_trip_keeps_order_widths_and_enabled_state() {
    let mut w = testing::window("view.mail");
    let value = json!({"enabled":true,"items":[{"column":"subject","width":390},{"column":"size","width":110}]});
    w.state_mut().execute("view.columns", value.clone()).unwrap();
    let prefs = w.state().prefs();
    let prefs = serde_json::from_value(serde_json::to_value(prefs).unwrap()).unwrap();
    let mut next = MailApp::new(
        mailcraft_engine::Session::at(mailcraft_engine::sample::mailbox(testing::now(), 0), testing::now(), 0),
        crate::Services::default(),
    );
    next.apply_prefs(prefs);
    assert_eq!(json!(next.session.prefs.columns), value);
    next.execute("view.reset", json!({})).unwrap();
    assert!(!next.session.prefs.columns.enabled);
}

#[test]
fn rendered_cells_align_with_headers_and_the_scrollbar_stays_on_screen() {
    let mut mailbox = mailcraft_engine::sample::mailbox(testing::now(), 0);
    let folder = mailbox.inbox().unwrap();
    let account = mailbox.default_account().unwrap().id;
    for i in 0..80 {
        mailbox
            .add_message(Message { folder, account, date: testing::now() - i, subject: format!("Synthetic column row {i}"), ..Default::default() })
            .unwrap();
    }
    let mut w = testing::window_on(mailbox, "view.mail");
    // The scroll bar in a lane of its own, as off a Mac (on one it floats over the rows).
    w.state_mut().overlay_scroll_bars = false;
    w.state_mut().execute("ui.dismissReminder", json!({"all":true})).unwrap();
    w.state_mut().execute("view.columns", json!({"enabled":true})).unwrap();
    w.run_steps(4);
    let check = |w: &testing::Window| {
        let id = w
            .state()
            .session
            .mailbox
            .messages
            .keys()
            .copied()
            .find(|id| w.state().hit.rect("messageColumn", id * 16 + Column::Subject as u64).is_some())
            .unwrap();
        for column in [Column::From, Column::Subject, Column::Date] {
            let head = w.state().hit.rect("columnHeader", column as u64).unwrap();
            let body = w.state().hit.rect("messageColumn", id * 16 + column as u64).unwrap();
            assert!((head.min.x - body.min.x - 4.0).abs() < 0.1);
            assert!((body.max.x - head.max.x - 4.0).abs() < 0.1);
        }
        let scrollbar = w.state().hit.rect("listScroll", 0).unwrap();
        let sort = w.state().hit.rect("listSort", 1).unwrap();
        assert!(scrollbar.max.x <= sort.max.x + 50.0, "{scrollbar:?} outside {sort:?}");
    };
    check(&w);
    w.state_mut().execute("view.readingPane", json!({"value":"off"})).unwrap();
    w.run_steps(4);
    check(&w);
}

#[test]
fn resizing_a_column_changes_width_without_sorting() {
    use egui::{Event, Modifiers};
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all":true})).unwrap();
    w.state_mut().execute("view.readingPane", json!({"value":"off"})).unwrap();
    w.state_mut().execute("view.columns", json!({"enabled":true})).unwrap();
    w.run_steps(3);
    let sort = (w.state().session.view.sort, w.state().session.view.sort_descending);
    let before = w.state().session.prefs.columns.items.iter().find(|c| c.column == Column::From).unwrap().width;
    let start = w.state().hit.rect("columnResize", Column::From as u64).unwrap().center();
    w.input_mut().events.push(Event::PointerMoved(start));
    w.step();
    w.input_mut().events.push(Event::PointerButton { pos: start, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    w.step();
    let end = start + vec2(55.0, 0.0);
    w.input_mut().events.push(Event::PointerMoved(end));
    w.step();
    w.input_mut().events.push(Event::PointerButton { pos: end, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    w.step();
    let after = w.state().session.prefs.columns.items.iter().find(|c| c.column == Column::From).unwrap().width;
    assert!(after > before, "{before} -> {after}");
    assert_eq!((w.state().session.view.sort, w.state().session.view.sort_descending), sort);
}

/// Press at `from`, move to `to` over a few frames, and let go.
fn drag(w: &mut testing::Window, from: egui::Pos2, to: egui::Pos2) {
    use egui::{Event, Modifiers};
    w.run_steps(45);
    w.input_mut().events.push(Event::PointerMoved(from));
    w.step();
    w.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    w.step();
    for k in 1..=6 {
        w.input_mut().events.push(Event::PointerMoved(from + (to - from) * (k as f32 / 6.0)));
        w.step();
    }
    w.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    w.run_steps(2);
}

fn shown(w: &testing::Window) -> Vec<Column> {
    w.state().session.prefs.columns.items.iter().map(|c| c.column).collect()
}

fn press(w: &mut testing::Window, kind: &str, id: u64) {
    let at = w.state().hit.rect(kind, id).unwrap_or_else(|| panic!("no {kind} {id}")).center();
    testing::click(w, at, PointerButton::Primary, 1);
}

#[test]
fn show_columns_adds_moves_and_removes_as_outlooks_does() {
    use Column::*;
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all":true})).unwrap();
    assert!(!w.state().session.prefs.columns.enabled);
    w.state_mut().execute("ui.dialog", json!({"name": "showColumns"})).unwrap();
    w.run_steps(2);
    assert_eq!(testing::dialog(&w), Some("showColumns"));
    // Size from the left goes to the end on the right, then up one.
    press(&mut w, "columnAvailable", Size as u64);
    press(&mut w, "columnsAdd", 0);
    press(&mut w, "columnShown", Size as u64);
    press(&mut w, "columnsUp", 0);
    // From goes back to the left; Subject can't.
    press(&mut w, "columnShown", From as u64);
    press(&mut w, "columnsRemove", 0);
    press(&mut w, "columnShown", Subject as u64);
    press(&mut w, "columnsRemove", 0);
    // Nothing changes before OK.
    assert_eq!(shown(&w), [Importance, Attachments, From, Subject, Date, Flag]);
    w.get_by_label("OK").click();
    w.run_steps(2);
    assert_eq!(testing::dialog(&w), None);
    assert_eq!(shown(&w), [Importance, Attachments, Subject, Date, Size, Flag]);
    assert!(w.state().session.prefs.columns.enabled, "OK shows the columns");
}

#[test]
fn cancel_leaves_the_columns_as_they_were() {
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all":true})).unwrap();
    let before = w.state().session.prefs.columns.clone();
    w.state_mut().execute("ui.dialog", json!({"name": "showColumns"})).unwrap();
    w.run_steps(2);
    press(&mut w, "columnAvailable", Column::Size as u64);
    press(&mut w, "columnsAdd", 0);
    w.get_by_label("Cancel").click();
    w.run_steps(2);
    assert_eq!(testing::dialog(&w), None);
    assert_eq!(w.state().session.prefs.columns, before);
}

#[test]
fn a_heading_dragged_along_moves_and_one_dragged_off_goes() {
    use Column::*;
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all":true})).unwrap();
    w.state_mut().execute("view.readingPane", json!({"value":"off"})).unwrap();
    w.state_mut().execute("view.columns", json!({"enabled":true})).unwrap();
    w.run_steps(3);
    let sort = (w.state().session.view.sort, w.state().session.view.sort_descending);
    let head = |w: &testing::Window, c: Column| w.state().hit.rect("columnHeader", c as u64).unwrap();
    // From, dropped past Date's middle, goes after Date, though the drag strayed onto the rows.
    let (from, date) = (head(&w, From), head(&w, Date));
    drag(&mut w, from.center(), pos2(date.max.x - 5.0, date.max.y + 30.0));
    assert_eq!(shown(&w), [Importance, Attachments, Subject, Date, From, Flag]);
    // Flag, dragged down off the row, is taken away; Subject is not.
    let flag = head(&w, Flag).center();
    drag(&mut w, flag, flag + vec2(0.0, 120.0));
    assert_eq!(shown(&w), [Importance, Attachments, Subject, Date, From]);
    let subject = head(&w, Subject).center();
    drag(&mut w, subject, subject + vec2(0.0, 120.0));
    assert_eq!(shown(&w), [Importance, Attachments, Subject, Date, From]);
    assert_eq!((w.state().session.view.sort, w.state().session.view.sort_descending), sort, "a drag doesn't sort");
}

#[test]
fn a_slow_drag_resizes_as_far_as_it_went() {
    use egui::{Event, Modifiers};
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all":true})).unwrap();
    w.state_mut().execute("view.readingPane", json!({"value":"off"})).unwrap();
    w.state_mut().execute("view.columns", json!({"enabled":true})).unwrap();
    w.run_steps(3);
    let before = w.state().session.prefs.columns.items.iter().find(|c| c.column == Column::From).unwrap().width;
    let start = w.state().hit.rect("columnResize", Column::From as u64).unwrap().center();
    w.input_mut().events.push(Event::PointerMoved(start));
    w.step();
    w.input_mut().events.push(Event::PointerButton { pos: start, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    w.step();
    // Less than half a point a frame, 40 points in all.
    let mut at = start;
    for _ in 0..100 {
        at += vec2(0.4, 0.0);
        w.input_mut().events.push(Event::PointerMoved(at));
        w.step();
    }
    w.input_mut().events.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    w.step();
    let after = w.state().session.prefs.columns.items.iter().find(|c| c.column == Column::From).unwrap().width;
    assert!((i32::from(after) - i32::from(before) - 40).abs() <= 1, "{before} -> {after}");
}

#[test]
fn sent_items_name_who_mail_went_to() {
    let mut w = testing::window("view.mail");
    let app = w.state();
    let account = app.session.mailbox.default_account().unwrap().id;
    let sent = app.session.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Sent).unwrap();
    let m = app.session.mailbox.messages.values().find(|m| m.folder == sent && !m.to.is_empty()).expect("the sample has sent mail").clone();
    assert_eq!(super::super::who(&app.session, &m), m.to.iter().map(|a| a.display()).collect::<Vec<_>>().join("; "));
    assert!(!outgoing(app));
    assert_eq!(label(Column::From, outgoing(app)), "From");
    w.state_mut().execute("folder.select", json!({"id": sent})).unwrap();
    assert!(outgoing(w.state()));
    assert_eq!(label(Column::From, true), "To");
}

#[test]
fn quick_actions_stay_in_view_when_the_columns_run_past_the_pane() {
    let mut w = testing::window("view.mail");
    w.state_mut().overlay_scroll_bars = false;
    w.state_mut().execute("ui.dismissReminder", json!({"all":true})).unwrap();
    w.state_mut().execute("view.columns", json!({"enabled":true})).unwrap();
    w.run_steps(4);
    let id = w.state_mut().session.rows().first().unwrap().id;
    let row = w.state().hit.rect("message", id).unwrap();
    // The scroll bar's lane, which the rows end before.
    let edge = w.state().hit.rect("listScroll", 0).unwrap().min.x;
    assert!(row.max.x > edge + 50.0, "the columns are wider than the pane: {row:?}, {edge}");
    w.input_mut().events.push(egui::Event::PointerMoved(pos2(row.min.x + 60.0, row.center().y)));
    w.run_steps(3);
    let (flag, delete) = (w.state().hit.rect("messageFlag", id).unwrap(), w.state().hit.rect("messageDelete", id).unwrap());
    assert!(flag.max.x <= edge && delete.min.x > row.min.x, "{flag:?} {delete:?} past {edge}");
}
