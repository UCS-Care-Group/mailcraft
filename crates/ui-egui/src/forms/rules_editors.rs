// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The dialogs Rules and Alerts opens over its list: the rule builder (New Rule…, Change Rule,
//! and a filter of a webmail's Sieve script, #9999) and the Sieve editor (a Sieve rule, or the
//! owner's own script whole, #9999). Each is modal over the list, as Appointment Recurrence is
//! over an appointment: the list stays as it was under it, Escape is Cancel (when it is on top: a
//! list open in it closes first, #406), and Enter in one of the builder's one-line fields, or with
//! none, is OK when the rule has all it needs.

use egui::vec2;
use mailcraft_engine::cmd::rules::NotOnServer;

use super::kit::{Pass, error, error_room, field_id, footer};
use super::rules::RulesForm;
use crate::MailApp;
use crate::sieve_editor::EditorAsks;
use crate::theme::{Tokens, regular, semibold};

/// How the owner left the builder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Left {
    Ok,
    Cancel,
}

/// What a dialog over the list leaves of the window's height for its fields: room for its title,
/// its buttons and the window's edges.
const AROUND: f32 = 170.0;

/// The id of the builder's dialog.
fn builder_id() -> egui::Id {
    field_id("rule_builder_dialog")
}

/// The id of the Sieve editor's dialog.
fn sieve_id() -> egui::Id {
    field_id("sieve_editor_dialog")
}

/// The frame of a dialog over the list, and its width: at most `most`, within the window.
fn frame(ctx: &egui::Context, most: f32) -> (egui::Frame, f32) {
    let t = Tokens::get(ctx);
    let frame = egui::Frame::window(&ctx.global_style()).fill(t.surface).inner_margin(egui::Margin::same(16));
    (frame, (ctx.content_rect().width() - 48.0).clamp(160.0, most))
}

/// The height the fields of a dialog over the list may take before they scroll.
fn room(ctx: &egui::Context, below: f32) -> f32 {
    let room = ctx.content_rect().height() - AROUND - below;
    if room.is_finite() { room.max(120.0) } else { 120.0 }
}

/// The rule builder, over the list: its name, conditions, exceptions, actions and where it is
/// kept, then Add Rule (or Save) and Cancel. How it was left, once it was.
pub(super) fn builder(
    f: &mut RulesForm,
    ctx: &egui::Context,
    app: &MailApp,
    categories: &[String],
    place: Option<&(String, Option<NotOnServer>)>,
    held: bool,
) -> Option<Left> {
    let t = Tokens::get(ctx);
    let (frame, width) = frame(ctx, 600.0);
    let id = builder_id();
    let mut left = None;
    let mut enter = false;
    let mut ready = false;
    egui::Modal::new(id).frame(frame).backdrop_color(t.scrim).show(ctx, |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        let title = if f.editing.is_some() { tl!("Change rule") } else { tl!("New rule") };
        ui.label(egui::RichText::new(title).font(semibold(14.0)));
        let mut pass = Pass::new(ui, &app.session, &mut false);
        let below = error_room(ui, &f.error);
        let mut lacks = None;
        egui::ScrollArea::vertical().id_salt("rule_builder_body").max_height(room(ctx, below)).auto_shrink([false, true]).show(ui, |ui| {
            // Room for the scroll bar beside the fields.
            let margin = egui::Margin { left: 0, right: 12, top: 1, bottom: 2 };
            egui::Frame::NONE.inner_margin(margin).show(ui, |ui| lacks = f.builder(ui, &mut pass, categories, place, held));
        });
        // What the rule still needs, wrapped within the dialog: it can be long.
        if let Some(why) = lacks {
            ui.label(egui::RichText::new(why).font(regular(11.5)).color(t.text_dim));
        }
        error(ui, &f.error);
        ready = lacks.is_none();
        let label = if f.editing.is_some() { tl!("Save") } else { tl!("Add Rule") };
        let pressed = footer(ui, label, ready, false);
        if pressed.cancel {
            left = Some(Left::Cancel);
        } else if pressed.ok && ready {
            left = Some(Left::Ok);
        }
        enter = pass.submit;
    });
    let escape = crate::keys::escape(ctx, crate::keys::modal_layer(id));
    left.or(if escape {
        Some(Left::Cancel)
    } else if enter && ready {
        Some(Left::Ok)
    } else {
        None
    })
}

/// The Sieve editor, over the list, when it is open: a Sieve rule, or the owner's own script
/// whole. What it asks of the Rules dialog, if anything; Escape asks for Cancel.
pub(super) fn sieve(f: &mut RulesForm, ctx: &egui::Context, app: &MailApp) -> Option<EditorAsks> {
    let editor = f.sieve.as_mut()?;
    let t = Tokens::get(ctx);
    let (frame, width) = frame(ctx, 680.0);
    let id = sieve_id();
    let mut asks = None;
    egui::Modal::new(id).frame(frame).backdrop_color(t.scrim).show(ctx, |ui| {
        ui.set_width(width);
        // (The editor's text has a scroll area of its own, as high as the window leaves room for.)
        asks = editor.ui(ui, app);
    });
    let escape = crate::keys::escape(ctx, crate::keys::modal_layer(id));
    asks.or(escape.then_some(EditorAsks::Cancel))
}
