// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1174: the switch between Mail, Calendar, People and Tasks at the foot of the pane on the
//! left, in place of the bar of icons down the window's edge.

use egui::PointerButton;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use serde_json::json;

use crate::people_view::testing::{self, Window};

fn window() -> Window {
    let mut w = testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.run_steps(3);
    w
}

fn at_foot(w: &mut Window, foot: bool) {
    let value = if foot { "foot" } else { "bar" };
    w.state_mut().execute("view.navigationBar", json!({"value": value})).unwrap();
    w.run_steps(3);
}

/// The row at the foot, by the hit map: one rectangle for each module, side by side.
fn row(w: &Window) -> Vec<egui::Rect> {
    (0..4).filter_map(|i| w.state().hit.rect("moduleFoot", i)).collect()
}

/// Where the modules are: the bar down the left edge, or the row at the foot of the pane.
#[test]
fn the_switch_moves_from_the_bar_to_the_foot_and_back() {
    let mut w = window();
    assert!(row(&w).is_empty(), "the bar is the one in use");
    let wide = w.state().session.view.folder_pane;
    // Where the pane starts with the bar beside it, to see the bar's column go (#1174).
    let with_bar = w.state().hit.rect("folderPaneHide", 0).expect("the folder pane's hide button");
    at_foot(&mut w, true);
    let foot = row(&w);
    assert_eq!(foot.len(), 4, "a place for each module");
    // Side by side, in one row, each as wide as the next.
    assert!(foot.windows(2).all(|p| p[1].min.x >= p[0].max.x - 0.5 && (p[1].height() - p[0].height()).abs() < 0.5), "{foot:?}");
    assert!(foot.windows(2).all(|p| (p[0].min.y - p[1].min.y).abs() < 0.5));
    // At the foot of the pane on the left, not across the window: the button that hides the pane
    // stands over the row, and the row ends well left of the window's middle.
    let window = w.state().hit.rect("folderPaneHide", 0).expect("the folder pane's hide button");
    // (The window the test harness builds: 1280 by 800.)
    let (width, height) = (1280.0, 800.0);
    assert!(foot[0].min.x < 40.0 && foot[3].max.x < width / 2.0, "{foot:?}");
    assert!(foot[0].min.y > height / 2.0, "at the foot");
    assert!(window.max.x <= foot[3].max.x + 1.0 && window.max.y < foot[0].min.y, "the row is under the pane, {window:?}");
    // The bar's column is gone, so the pane and everything beside it start further left.
    assert!(window.min.x < with_bar.min.x - 40.0, "{window:?} against {with_bar:?}");
    at_foot(&mut w, false);
    assert!(row(&w).is_empty(), "the bar is back");
    assert!((w.state().hit.rect("folderPaneHide", 0).unwrap().min.x - with_bar.min.x).abs() < 0.5, "the bar's column is back");
    assert_eq!(w.state().session.view.folder_pane, wide, "the folder pane is untouched");
}

/// A click on one of them goes to that module, and the row is there in every module.
#[test]
fn a_click_at_the_foot_goes_to_that_module() {
    let mut w = window();
    at_foot(&mut w, true);
    for (i, module) in
        [mailcraft_engine::Module::Calendar, mailcraft_engine::Module::People, mailcraft_engine::Module::Tasks, mailcraft_engine::Module::Mail]
            .into_iter()
            .enumerate()
    {
        let at = [1u64, 2, 3, 0][i];
        let r = w.state().hit.rect("moduleFoot", at).unwrap_or_else(|| panic!("{module:?} at the foot"));
        testing::click(&mut w, r.center(), PointerButton::Primary, 1);
        assert_eq!(w.state().session.view.module, module);
        assert_eq!(row(&w).len(), 4, "{module:?} shows the row too");
    }
}

/// Each one is a button that says whether its module is the one in view, for a screen reader.
#[test]
fn a_screen_reader_hears_which_module_is_in_view() {
    let mut w = window();
    at_foot(&mut w, true);
    let picked = |w: &Window, name: &str| {
        let node = w
            .query_all(egui_kittest::kittest::By::new().role(egui::accesskit::Role::Button).label(name))
            .next()
            .unwrap_or_else(|| panic!("{name} is a button"));
        node.accesskit_node().data().is_selected()
    };
    assert_eq!(picked(&w, "Mail"), Some(true));
    assert_eq!(picked(&w, "Calendar"), Some(false));
    w.state_mut().execute("view.calendar", json!({})).unwrap();
    w.run_steps(3);
    assert_eq!(picked(&w, "Calendar"), Some(true));
}

/// The pictures for the pull request, in both themes: the row at the foot, in Mail and in the
/// calendar, when `MAILCRAFT_SHOTS_DIR` names a folder.
#[test]
fn shots() {
    let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") else { return };
    let dir = std::path::Path::new(&dir);
    let mut w = window();
    at_foot(&mut w, true);
    let save = |w: &mut Window, name: &str| {
        for theme in ["light", "dark"] {
            w.state_mut().execute("ui.theme", json!({"value": theme})).unwrap();
            w.run_steps(4);
            w.render().unwrap().save(dir.join(format!("{name}-{theme}.png"))).unwrap();
        }
    };
    save(&mut w, "nav-foot-mail");
    w.state_mut().execute("view.calendar", json!({})).unwrap();
    w.run_steps(4);
    save(&mut w, "nav-foot-calendar");
}

/// The row sits at the foot of the pane on the left, so where the module in view has no pane the
/// bar comes back: the switch is never out of reach. Mail and the calendar can hide theirs;
/// People and Tasks always have one.
#[test]
fn the_bar_comes_back_where_there_is_no_pane_for_the_row() {
    let mut w = window();
    at_foot(&mut w, true);
    let with_row = w.state().hit.rect("folderPaneHide", 0).expect("the folder pane's hide button");
    assert_eq!(row(&w).len(), 4);
    // Mail with the folder pane hidden: the strip left is too narrow for four modules.
    w.state_mut().execute("view.folderPane", json!({"value": false})).unwrap();
    w.run_steps(3);
    assert!(row(&w).is_empty(), "no row squeezed into the strip");
    // The bar is back beside the strip, so there is still a way to the other modules: the strip
    // starts a bar's width from the window's edge.
    let strip = w.state().hit.rect("folderPaneShow", 0).expect("the strip is there");
    assert!(strip.min.x > 40.0, "the bar stands beside it, {strip:?}");
    // The calendar has no pane at all then, and must still be left by hand.
    w.state_mut().execute("view.calendar", json!({})).unwrap();
    w.run_steps(3);
    assert!(row(&w).is_empty());
    // People and Tasks keep theirs, so the row is at their foot.
    for module in ["view.people", "view.tasks"] {
        w.state_mut().execute(module, json!({})).unwrap();
        w.run_steps(3);
        assert_eq!(row(&w).len(), 4, "{module}");
    }
    // The pane back: Mail has the row again, where it was.
    w.state_mut().execute("view.mail", json!({})).unwrap();
    w.state_mut().execute("view.folderPane", json!({"value": true})).unwrap();
    w.run_steps(3);
    assert_eq!(row(&w).len(), 4);
    assert!((w.state().hit.rect("folderPaneHide", 0).unwrap().min.x - with_row.min.x).abs() < 0.5);
}

/// The choice carries over a restart, as the other layout choices do.
#[test]
fn the_choice_carries_over_a_restart() {
    let mut w = window();
    at_foot(&mut w, true);
    let saved = w.state().prefs();
    let mut next = testing::window("view.mail");
    assert!(!next.state().session.view.nav_foot, "a fresh window starts with the bar");
    next.state_mut().apply_prefs(saved);
    next.run_steps(3);
    assert!(next.state().session.view.nav_foot, "the foot was asked for");
    assert_eq!(row(&next).len(), 4);
}

/// The names show beside the icons only where the pane is wide enough for the longest of them,
/// and then for all four: a narrow pane leaves the icons alone rather than naming some of them.
#[test]
fn the_names_show_where_they_all_fit() {
    // A module's name at 12 pt is about 50 points wide; each module has a quarter of the pane.
    let widest = 50.0;
    assert!(!crate::nav::names_fit(230.0 / 4.0, widest), "a pane at its usual width: the icons alone");
    assert!(crate::nav::names_fit(360.0 / 4.0, widest), "a pane dragged wider: the names too");
    // The rule is the same for every module, so the row is never half named.
    assert_eq!(crate::nav::names_fit(72.0, widest), crate::nav::names_fit(72.0, widest));
    assert!(!crate::nav::names_fit(widest + 21.0, widest) && crate::nav::names_fit(widest + 22.0, widest), "the icon and the gap beside it");
}
