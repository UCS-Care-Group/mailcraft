// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! View Source: what `mail.viewSource` answered, shown as plain text in a fixed-width font to
//! read, select and copy. Nothing of it is rendered, followed or fetched: a link in it is text
//! like the rest, and what can't be seen was made visible by the engine.

use egui::{Ui, vec2};
use serde_json::Value;

use crate::MailApp;
use crate::theme::Tokens;

/// The source of one message, as the dialog shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    /// The message's subject, for the title.
    pub subject: String,
    pub text: String,
    /// Only the first part of it is shown.
    pub truncated: bool,
}

impl Source {
    /// The dialog for what `mail.viewSource` answered.
    pub fn from_answer(v: &Value) -> Option<Source> {
        Some(Source {
            subject: mailcraft_names::shown(v.get("subject").and_then(Value::as_str).unwrap_or_default(), 80),
            text: v.get("source")?.as_str()?.to_string(),
            truncated: v.get("truncated").and_then(Value::as_bool) == Some(true),
        })
    }
}

impl MailApp {
    /// After the owner ran View Source: its window, with what it answered.
    pub(crate) fn show_source(&mut self, answer: &Value) {
        if let Some(source) = Source::from_answer(answer) {
            self.dialog = Some(crate::dialogs::Dialog::ViewSource(Box::new(source)));
        }
    }
}

/// The dialog's content; whether Close was pressed.
pub(crate) fn show(ui: &mut Ui, source: &Source, t: &Tokens) -> bool {
    let mut close = false;
    ui.label(egui::RichText::new(tl!("Shown as text: nothing in it runs, opens or is downloaded.")).color(t.text_dim).size(11.5));
    if source.truncated {
        ui.label(egui::RichText::new(tl!("The source is long: only its first megabyte is shown.")).color(t.text_dim).size(11.5));
    }
    egui::Frame::new().stroke(egui::Stroke::new(1.0, t.border)).fill(t.panel).inner_margin(6.0).show(ui, |ui| {
        egui::ScrollArea::both().max_height(420.0).auto_shrink([false, false]).show(ui, |ui| {
            // Read only, selectable, and never a link.
            let mut text = source.text.as_str();
            ui.add(egui::TextEdit::multiline(&mut text).code_editor().desired_width(f32::INFINITY).frame(egui::Frame::NONE));
        });
    });
    ui.add_space(4.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.add(egui::Button::new(tl!("Close")).min_size(vec2(72.0, 26.0))).clicked() {
            close = true;
        }
        if ui.add(egui::Button::new(tl!("Copy")).min_size(vec2(72.0, 26.0))).on_hover_text(tl!("Copy all of it")).clicked() {
            ui.ctx().copy_text(source.text.clone());
        }
    });
    close
}
