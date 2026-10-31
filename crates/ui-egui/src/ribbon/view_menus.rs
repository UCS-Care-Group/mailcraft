// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The View tab's menus for how the mail looks: the list's ready-made views, the zoom of the
//! message being read, and the searches run lately. Each entry runs a command.

use egui::{Ui, vec2};
use mailcraft_engine::cmd::view::display::ListView;
use serde_json::json;

use super::Menu;
use crate::MailApp;
use crate::widgets::{mi, mi_check};

/// The zooms the menu offers by name, in percent (steps of `view.zoomIn`, so In and Out go on from them).
const ZOOM_CHOICES: [u16; 6] = [75, 100, 125, 150, 200, 300];

/// How many characters of a recent search its entry shows.
const SHOWN_CHARS: usize = 60;

pub(super) fn show(ui: &mut Ui, app: &mut MailApp, menu: Menu) {
    match menu {
        Menu::ChangeView => {
            let now = ListView::of(&app.session);
            for v in ListView::ALL {
                mi_check(ui, app, v.label(), now == Some(v), "view.changeView", json!({"value": v.code()}));
            }
        }
        Menu::Zoom => zoom(ui, app),
        Menu::RecentSearches => recent(ui, app),
        _ => {}
    }
}

fn zoom(ui: &mut Ui, app: &mut MailApp) {
    mi(ui, app, "Zoom In", "view.zoomIn", json!({}));
    mi(ui, app, "Zoom Out", "view.zoomOut", json!({}));
    mi(ui, app, "Actual Size", "view.zoomReset", json!({}));
    ui.separator();
    let now = app.session.prefs.zoom_percent();
    for z in ZOOM_CHOICES {
        let checked = now == z;
        // A number reads the same in every language.
        let mark = egui::RichText::new("✓");
        let mark = if checked { mark } else { mark.color(egui::Color32::TRANSPARENT) };
        if ui.add(egui::Button::new((mark, format!("{z}%"))).selected(checked).min_size(vec2(190.0, 0.0))).clicked() {
            let _ = app.run("view.zoom", json!({"value": z}));
            ui.close();
        }
    }
}

fn recent(ui: &mut Ui, app: &mut MailApp) {
    let searches: Vec<String> = app.session.prefs.recent_searches().map(str::to_string).collect();
    if searches.is_empty() {
        ui.add_enabled(false, egui::Button::new(tl!("No recent searches")).min_size(vec2(190.0, 0.0)));
    }
    for query in searches {
        // The owner's own words, as typed: never through the catalogue.
        let shown: String = match query.char_indices().nth(SHOWN_CHARS) {
            Some((at, _)) => format!("{}…", query.get(..at).unwrap_or(&query)),
            None => query.clone(),
        };
        if ui.add(egui::Button::new(shown).min_size(vec2(190.0, 0.0))).clicked() {
            app.search_text = query.clone();
            let _ = app.run("view.search", json!({"query": query}));
            let _ = app.execute("view.recentSearches", json!({"remember": query}));
            ui.close();
        }
    }
    ui.separator();
    let any = app.session.prefs.recent_searches().next().is_some();
    if ui.add_enabled(any, egui::Button::new(tl!("Clear Recent Searches")).min_size(vec2(190.0, 0.0))).clicked() {
        let _ = app.run("view.recentSearches", json!({"clear": true}));
        ui.close();
    }
}
