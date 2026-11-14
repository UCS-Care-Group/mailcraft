// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Recent attachments, as a menu the owner can open while writing a message.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use egui::{Context, Ui};

use crate::MailApp;

pub type Loader = Box<dyn Fn(&Context) -> Receiver<Vec<PathBuf>>>;
pub(super) const ARROW: f32 = 14.0;

enum Loading {
    Pending(Receiver<Vec<PathBuf>>),
    Ready(Vec<PathBuf>),
}

struct State {
    pass: u64,
    loading: Loading,
}

type Cache = Arc<Mutex<Option<State>>>;

#[cfg(test)]
mod tests;

pub(super) fn available(app: &MailApp) -> bool {
    let watch = app.secret_watch();
    !watch.driven && !watch.captured
}

pub(super) fn arrow(ui: &mut Ui, app: &mut MailApp) {
    let label = tl!("Recent files");
    let enabled = available(app) && app.enabled("ui.attach");
    let response = crate::widgets::icon_button(ui, "dropdown", label, ARROW, enabled);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    if enabled {
        egui::Popup::menu(&response).show(|ui| contents(ui, app));
    }
}

pub(super) fn contents(ui: &mut Ui, app: &mut MailApp) {
    let (Some(load), Some(compose)) = (&app.services.recent_attachments, app.session.compose_id()) else { return };
    if !available(app) {
        // Even an agent allowed to click or take a screenshot can't read or select these paths.
        return;
    }
    ui.set_min_width(210.0);
    let pass = ui.ctx().cumulative_pass_nr();
    let cache_id = ui.scope_id().with(("recent_attachments", app.session.openings(), compose));
    let cache = ui.ctx().data_mut(|data| data.get_temp_mut_or_default::<Cache>(cache_id).clone());
    if let Ok(mut cache) = cache.lock() {
        if cache.as_ref().is_none_or(|state| pass.saturating_sub(state.pass) > 1) {
            *cache = Some(State { pass, loading: Loading::Pending(load(ui.ctx())) });
        }
        if let Some(state) = cache.as_mut() {
            state.pass = pass;
            if let Loading::Pending(receiver) = &state.loading {
                match receiver.try_recv() {
                    Ok(paths) => state.loading = Loading::Ready(paths),
                    Err(TryRecvError::Disconnected) => state.loading = Loading::Ready(Vec::new()),
                    Err(TryRecvError::Empty) => {}
                }
            }
            match &state.loading {
                Loading::Pending(_) => {
                    ui.spinner();
                }
                Loading::Ready(paths) if paths.is_empty() => {
                    ui.label(tl!("No recent files"));
                }
                Loading::Ready(paths) => {
                    for path in paths.iter().take(10) {
                        let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
                        let Some(full) = path.to_str() else { continue };
                        let label = mailcraft_engine::opening::file_name(name);
                        if ui.button(label).clicked() {
                            if let Err(error) = crate::files::attach_all(app, &[full.to_owned()], Some(compose)) {
                                app.status(error);
                            }
                            ui.close();
                        }
                    }
                }
            }
        }
    }
    ui.separator();
    if ui.button(tl!("Browse This PC…")).clicked() {
        let _ = app.run("ui.attach", serde_json::json!({}));
        ui.close();
    }
}
