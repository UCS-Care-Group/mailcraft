// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The size of the picture selected in the message, in pixels (#1310): its width, and the
//! height that follows from it. A number typed in is set when it is entered, one dragged when
//! the drag ends, so that each is one step of the message's undo.

use egui::{Ui, vec2};
use mailcraft_model::pictures::{FIRST_ID, MAX_SHOWN_WIDTH};
use serde_json::json;

use crate::MailApp;
use crate::theme::Tokens;

/// Along the foot of the message while one of its own pictures is the selection, alone.
pub(super) fn show(app: &mut MailApp, ui: &mut Ui) {
    let Some((id, set, [w, h])) = selected(app, ui.ctx()) else { return };
    let t = Tokens::get(ui.ctx());
    let most = MAX_SHOWN_WIDTH;
    let dragged_id = egui::Id::unique("mailcraft-picture-size").with(id);
    let shown = shown_width(set, w, most);
    let mut width = ui.ctx().data(|d| d.get_temp::<u16>(dragged_id)).unwrap_or(shown);
    let tall = |width: u16| (f32::from(width) * h / w).round().max(1.0) as u32;
    let mut height = tall(width);
    let mut asked: Option<Option<u16>> = None;
    egui::Panel::bottom(egui::Id::unique("compose_picture_size"))
        .exact_size(34.0)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(t.panel).stroke(egui::Stroke::new(1.0, t.border)).inner_margin(egui::Margin {
            left: 16,
            right: 12,
            top: 0,
            bottom: 0,
        }))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 0.0);
                ui.label(egui::RichText::new(tl!("Picture size")).color(t.text_dim));
                ui.add_space(6.0);
                ui.label(tl!("Width"));
                let wide = crate::text_menu::field(
                    ui.add(egui::DragValue::new(&mut width).range(8..=most).speed(1.0).update_while_editing(false).suffix(" px")),
                );
                ui.add_space(6.0);
                ui.label(tl!("Height"));
                let high = crate::text_menu::field(
                    ui.add(egui::DragValue::new(&mut height).range(tall(8)..=tall(most)).speed(1.0).update_while_editing(false).suffix(" px")),
                );
                app.hit.add("pictureWidth", u64::from(id), wide.rect);
                app.hit.add("pictureHeight", u64::from(id), high.rect);
                if high.changed() {
                    width = ((height as f32) * w / h).round().clamp(8.0, f32::from(most)) as u16;
                }
                let dragging = wide.dragged() || high.dragged();
                if dragging {
                    ui.ctx().data_mut(|d| d.insert_temp(dragged_id, width));
                } else {
                    ui.ctx().data_mut(|d| d.remove_temp::<u16>(dragged_id));
                }
                let dropped = (wide.drag_stopped() || high.drag_stopped()) && width != shown;
                if dropped || ((wide.changed() || high.changed()) && !dragging) {
                    asked = Some(Some(width));
                }
                ui.add_space(6.0);
                let reset =
                    ui.add_enabled(set.is_some(), egui::Button::new(tl!("Reset Size"))).on_hover_text(tl!("Show the picture at its own size"));
                app.hit.add("pictureReset", u64::from(id), reset.rect);
                if reset.clicked() {
                    asked = Some(None);
                }
            });
        });
    if let Some(width) = asked
        && width != set
    {
        let _ = app.run("compose.picture.set", json!({"id": id, "width": width}));
    }
}

/// The width the box shows: the one given, or the picture's own, at most what a message shows.
fn shown_width(set: Option<u16>, own: f32, most: u16) -> u16 {
    set.unwrap_or_else(|| own.round().clamp(8.0, f32::from(most)) as u16).min(most)
}

/// The picture of the message that is the selection, alone: its id, the width it was given and
/// its own size. Not while a signature is being edited, nor in a plain-text message.
fn selected(app: &MailApp, ctx: &egui::Context) -> Option<(u32, Option<u16>, [f32; 2])> {
    if !app.enabled("compose.picture.set") {
        return None;
    }
    let c = app.session.compose.as_ref()?;
    let sel = c.body.selection();
    if sel.end() != sel.start() + 1 {
        return None;
    }
    let ids = c.body.text().slice(sel.start(), sel.end()).ok()?.pictures();
    let id = *ids.first().filter(|id| ids.len() == 1 && **id >= FIRST_ID)?;
    let p = c.pictures.iter().find(|p| p.id == id)?;
    Some((id, p.width, super::editor::own_size(ctx, p)))
}
