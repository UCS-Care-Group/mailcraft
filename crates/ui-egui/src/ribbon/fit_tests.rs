// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The classic ribbon fits its window (#709): the order groups give up their form in (pure), and
//! every tab at every width, at 100 % and 200 % interface size.

use std::collections::BTreeMap;

use egui::vec2;
use mailcraft_engine::Session;
use serde_json::{Value, json};

use super::fit::{Stage, fit};
use super::{MODE_W, describe, hit_id, last_fit};
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// Four groups: widths for Full, Small, Icons, Folded.
const W: [f32; 4] = [200.0, 150.0, 100.0, 60.0];

fn steps(available: f32, priority: &[u8]) -> Vec<Stage> {
    fit(&vec![W; priority.len()], priority, 0.0, available).stages
}

#[test]
fn nothing_changes_while_the_row_fits() {
    let f = fit(&[W, W], &[5, 5], 100.0, 1000.0);
    assert_eq!(f.stages, [Stage::Full, Stage::Full]);
    assert!(f.note && f.behind.is_empty());
}

#[test]
fn the_note_goes_before_any_group_changes() {
    // Two full groups and a note of 100 (and its gap) don't fit in 420, the groups alone do.
    let f = fit(&[W, W], &[5, 5], 100.0, 420.0);
    assert_eq!(f.stages, [Stage::Full, Stage::Full]);
    assert!(!f.note);
}

#[test]
fn the_lowest_priority_shrinks_first_and_of_two_alike_the_right_one() {
    // 3 groups of 200 = 600; 560 needs one group a step down.
    assert_eq!(steps(560.0, &[5, 5, 5]), [Stage::Full, Stage::Full, Stage::Small]);
    assert_eq!(steps(560.0, &[5, 1, 5]), [Stage::Full, Stage::Small, Stage::Full]);
    assert_eq!(steps(560.0, &[9, 9, 1]), [Stage::Full, Stage::Full, Stage::Small]);
    assert_eq!(steps(500.0, &[5, 5, 5]), [Stage::Full, Stage::Small, Stage::Small]);
}

#[test]
fn every_group_is_small_before_any_loses_its_labels_and_so_on() {
    // 450 is three groups Small; 300 is three Icons; 180 is three Folded.
    assert_eq!(steps(450.0, &[1, 5, 9]), [Stage::Small; 3]);
    assert_eq!(steps(440.0, &[1, 5, 9]), [Stage::Icons, Stage::Small, Stage::Small]);
    assert_eq!(steps(300.0, &[1, 5, 9]), [Stage::Icons; 3]);
    assert_eq!(steps(260.0, &[1, 5, 9]), [Stage::Folded, Stage::Icons, Stage::Icons]);
    assert_eq!(steps(180.0, &[1, 5, 9]), [Stage::Folded; 3]);
}

#[test]
fn what_still_does_not_fit_goes_behind_more_lowest_priority_first() {
    let f = fit(&[W, W, W], &[9, 1, 5], 0.0, 150.0);
    // 3 folded groups are 180; `…` takes 38 more. The lowest priority goes first, then the next.
    assert_eq!(f.behind, [1, 2]);
    assert_eq!(f.stages, [Stage::Folded; 3]);
    let f = fit(&[W, W, W], &[9, 1, 5], 0.0, 1.0);
    assert_eq!(f.behind, [0, 1, 2]);
}

#[test]
fn a_step_that_makes_nothing_narrower_is_left_out() {
    // A group whose folded button is no narrower than its icons stays as it is.
    let wide = [100.0, 90.0, 40.0, 70.0];
    let f = fit(&[wide], &[5], 0.0, 45.0);
    assert_eq!(f.stages, [Stage::Icons]);
}

#[test]
fn a_narrower_row_never_shows_a_group_earlier_than_a_wider_one() {
    let widths = [[300.0, 200.0, 120.0, 80.0], [100.0, 100.0, 60.0, 90.0], [500.0, 330.0, 100.0, 70.0], [50.0, 50.0, 50.0, 60.0]];
    let priority = [4, 8, 1, 6];
    let mut before: Vec<Stage> = vec![Stage::Full; 4];
    let mut behind_before = 0;
    for available in (0..=1100).rev().step_by(7) {
        let f = fit(&widths, &priority, 120.0, available as f32);
        for (now, was) in f.stages.iter().zip(&before) {
            assert!(now >= was, "{available}: {now:?} after {was:?}");
        }
        assert!(f.behind.len() >= behind_before, "{available}");
        before = f.stages;
        behind_before = f.behind.len();
    }
}

#[test]
fn hostile_inputs_do_not_panic() {
    let _ = fit(&[], &[], 0.0, 100.0);
    let _ = fit(&[W], &[], 50.0, f32::NAN);
    let _ = fit(&[W, W], &[1], f32::INFINITY, -5.0);
    let _ = fit(&[[f32::NAN; 4]], &[5], 0.0, 10.0);
}

type Window = egui_kittest::Harness<'static, MailApp>;

fn window(size_percent: u32) -> Window {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let mut harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(vec2(1600.0, 700.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.run_steps(4);
    let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    harness.state_mut().execute("ui.ribbonMode", json!({"value": "classic"})).unwrap();
    harness.state_mut().execute("ui.interfaceSize", json!({"value": size_percent})).unwrap();
    harness.run_steps(4);
    harness
}

/// What brings up every tab of every module, and the tab a message being written has.
fn every_tab() -> Vec<(String, Vec<(&'static str, Value)>)> {
    let mut out = Vec::new();
    for module in ["view.mail", "view.calendar", "view.people", "view.tasks"] {
        for tab in ["Home", "Send / Receive", "Folder", "View", "Help"] {
            out.push((format!("{module} › {tab}"), vec![(module, json!({})), ("ui.tab", json!({"tab": tab}))]));
        }
    }
    for tab in ["Message", "Insert", "Review"] {
        out.push((tab.to_string(), vec![("view.mail", json!({})), ("mail.new", json!({})), ("ui.tab", json!({"tab": tab}))]));
    }
    out
}

/// Every tab, at every width from 600 to 2000 in steps of 20, at 100 % and 200 %: everything is
/// inside the ribbon, every control is reachable, and as the window narrows no group is at an
/// earlier stage than it was.
#[test]
fn every_tab_fits_at_every_width_and_every_control_is_reachable() {
    let mut seen = [false; 5];
    for percent in [100, 200] {
        let mut harness = window(percent);
        for (name, commands) in every_tab() {
            for (command, params) in commands {
                harness.state_mut().run(command, params).unwrap();
            }
            let mut before: BTreeMap<&'static str, u8> = BTreeMap::new();
            for width in (600..=2000).rev().step_by(20) {
                harness.set_size(vec2(width as f32, 700.0));
                harness.run_steps(3);
                let name = format!("{name} at {width} px, {percent} %");
                let app = harness.state();
                let band = app.hit.rect("ribbonBand", 0).unwrap_or_else(|| panic!("{name}: no ribbon"));
                let right = band.max.x - MODE_W - 8.0 + 0.5;
                let plan = last_fit(&harness.ctx);
                let tab = describe(app);
                let inside =
                    |what: &str, r: egui::Rect| assert!(r.max.x <= right && r.min.x >= band.min.x, "{name}: {what} at {r:?}, ribbon ends at {right}");
                let mut any_behind = false;
                for group in tab.groups.iter().filter(|g| g.items.iter().any(|i| i.size != super::Size::Hidden)) {
                    let (_, stage, behind) = plan
                        .iter()
                        .find(|(title, _, _)| *title == group.title)
                        .copied()
                        .unwrap_or_else(|| panic!("{name}: {} has no place", group.title));
                    let rank = if behind { 4 } else { stage as u8 };
                    if let Some(s) = seen.get_mut(rank as usize) {
                        *s = true;
                    }
                    let was = before.insert(group.title, rank).unwrap_or(0);
                    assert!(rank >= was, "{name}: {} is at stage {rank} after {was} in a wider window", group.title);
                    if behind {
                        any_behind = true;
                        continue;
                    }
                    if stage == Stage::Folded {
                        let r = app
                            .hit
                            .rect("ribbonFold", hit_id(group.title))
                            .unwrap_or_else(|| panic!("{name}: {} isn't folded into a button", group.title));
                        inside(group.title, r);
                        continue;
                    }
                    for item in group.items.iter().filter(|i| i.size != super::Size::Hidden) {
                        let r = app.hit.rect("ribbon", hit_id(item.key)).unwrap_or_else(|| panic!("{name}: {} isn't drawn", item.key));
                        inside(item.key, r);
                        if stage != Stage::Full {
                            assert!((r.height() - 22.0).abs() < 0.5, "{name}: {} at {r:?}", item.key);
                        }
                    }
                }
                match app.hit.rect("ribbonMore", 0) {
                    Some(r) => {
                        assert!(any_behind, "{name}: `…` with nothing behind it");
                        inside("…", r);
                    }
                    None => assert!(!any_behind, "{name}: groups are behind `…`, which isn't there"),
                }
            }
        }
    }
    assert_eq!(seen, [true; 5], "not every stage was seen (full, small, icons, folded, behind …)");
}

/// The widths the plan works from are the widths the layout takes: with room to spare, each group
/// starts where the one before it was said to end.
#[test]
fn the_widths_are_the_widths_drawn() {
    let mut harness = window(100);
    harness.set_size(vec2(2400.0, 700.0));
    for (name, commands) in every_tab() {
        for (command, params) in commands {
            harness.state_mut().run(command, params).unwrap();
        }
        harness.run_steps(3);
        let app = harness.state();
        let tab = describe(app);
        let drawn: Vec<(&super::Group, f32)> = tab
            .groups
            .iter()
            .filter(|g| g.items.iter().any(|i| i.size != super::Size::Hidden))
            .filter_map(|g| g.items.iter().find_map(|i| app.hit.rect("ribbon", hit_id(i.key))).map(|r| (g, r.min.x)))
            .collect();
        for pair in drawn.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            let said = super::fit::group_widths(a.0, &|text, size| {
                harness.ctx.fonts_mut(|f| f.layout_no_wrap(text.to_string(), crate::theme::regular(size), egui::Color32::WHITE).size().x)
            })[0];
            assert!((b.1 - a.1 - said).abs() < 1.0, "{name}: {} was said to be {said} wide, and is {}", a.0.title, b.1 - a.1);
        }
    }
}

#[test]
fn search_commands_gives_way_before_the_tabs() {
    use super::fit::{SEARCH_GAP, SEARCH_ICON_W, Search, search};
    assert_eq!(search(200.0, 140.0), Search::Named);
    assert_eq!(search(140.0 + SEARCH_GAP, 140.0), Search::Named);
    assert_eq!(search(140.0, 140.0), Search::Icon);
    assert_eq!(search(SEARCH_ICON_W + SEARCH_GAP, 140.0), Search::Icon);
    assert_eq!(search(SEARCH_ICON_W, 140.0), Search::Hidden);
    assert_eq!(search(-10.0, 140.0), Search::Hidden);
    assert_eq!(search(f32::NAN, 140.0), Search::Hidden);
}

/// Search commands never covers a tab (#712): at every width from 600 to 2000, at 100 %, 150 %
/// and 200 %, on every set of tabs, it is to the right of the last one with room between them, or
/// not there; and both its forms are seen.
#[test]
fn search_commands_never_covers_a_tab() {
    let (mut named, mut icon) = (false, false);
    for percent in [100, 150, 200] {
        let mut harness = window(percent);
        for (name, commands) in every_tab() {
            for (command, params) in commands {
                harness.state_mut().run(command, params).unwrap();
            }
            for width in (600..=2000).rev().step_by(20) {
                harness.set_size(vec2(width as f32, 700.0));
                harness.run_steps(3);
                let name = format!("{name} at {width} px, {percent} %");
                let app = harness.state();
                let Some(search) = app.hit.rect("ribbonSearch", 0) else { continue };
                let tabs: Vec<egui::Rect> = super::tabs(app).iter().filter_map(|tab| app.hit.rect("ribbonTab", hit_id(tab))).collect();
                assert!(!tabs.is_empty(), "{name}: no tabs");
                let last = tabs.iter().map(|r| r.max.x).fold(f32::MIN, f32::max);
                assert!(search.min.x >= last + 7.5, "{name}: Search commands at {search:?}, the tabs end at {last}");
                if search.width() > 30.0 {
                    named = true;
                } else {
                    icon = true;
                }
            }
        }
    }
    assert!(named && icon, "Search commands was not seen both with its name ({named}) and as the magnifier alone ({icon})");
}
