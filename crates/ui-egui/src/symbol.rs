// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Insert › Symbol: a grid of common symbols, drawn as text in the window's own fonts (no font or
//! picture of anybody's), and a field for a character's code (`U+2192`, or a number). The dialog
//! only collects the choice: a symbol clicked, or the code and Insert, runs `insert.symbol`, which
//! refuses control and format characters and what shows nothing. It stays open for more.

use egui::{RichText, Ui, vec2};
use mailcraft_engine::cmd::format::SYMBOLS;
use serde_json::{Value, json};

use crate::MailApp;
use crate::dialogs::button_row;
use crate::theme::{Tokens, regular};
use crate::widgets::primary_button;

pub const WIDTH: f32 = 380.0;

/// What a symbol's cell is called in the hit map: its code point.
pub fn cell_id(c: char) -> u64 {
    u64::from(u32::from(c))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Symbol {
    /// A character's code as typed: `U+2192`, `2192` (hex) or a decimal number with `#` before it.
    pub code: String,
    pub error: String,
}

impl Symbol {
    /// The params `insert.symbol` takes for the code typed.
    fn params(&self) -> Option<Value> {
        let code = self.code.trim();
        if let Some(n) = code.strip_prefix('#') {
            return n.parse::<u32>().ok().map(|n| json!({"code": n}));
        }
        (!code.is_empty()).then(|| json!({"code": code}))
    }

    /// The dialog's contents; whether it is done.
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> bool {
        let t = Tokens::get(ui.ctx());
        let mut done = false;
        let mut asked: Option<Value> = None;
        egui::Grid::new("symbols").spacing(vec2(2.0, 2.0)).show(ui, |ui| {
            for (i, c) in SYMBOLS.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(36.0, 32.0), egui::Sense::click());
                let fill = if resp.hovered() { t.hover } else { t.surface };
                ui.painter().rect(r, 3.0, fill, egui::Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
                // The no-break space shows as its name's initials.
                let shown = if *c == '\u{a0}' { "NBSP".to_string() } else { c.to_string() };
                let size = if *c == '\u{a0}' { 9.0 } else { 17.0 };
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, shown, regular(size), t.text);
                app.hit.add("symbolCell", cell_id(*c), r);
                let name = format!("U+{:04X}", u32::from(*c));
                if resp.on_hover_text(name).clicked() {
                    asked = Some(json!({"char": c.to_string()}));
                }
                if (i + 1) % 9 == 0 {
                    ui.end_row();
                }
            }
        });
        ui.label(tl!("Character code"));
        let resp = ui.add(egui::TextEdit::singleline(&mut self.code).hint_text("U+2192").char_limit(10).desired_width(f32::INFINITY));
        app.hit.add("symbolCode", 0, crate::text_menu::field(resp).rect);
        ui.label(RichText::new(tl!("A code point written U+2192 or 2192, or a decimal number after #.")).color(t.text_dim).size(11.0));
        if !self.error.is_empty() {
            ui.label(RichText::new(self.error.as_str()).color(t.red));
        }
        ui.add_space(4.0);
        let typed = self.params();
        button_row(ui, |ui| {
            let b = ui.add(egui::Button::new(tl!("Close")).min_size(vec2(72.0, 26.0)));
            app.hit.add("symbolClose", 0, b.rect);
            done |= b.clicked();
            let b = primary_button(ui, tl!("Insert"), typed.is_some());
            app.hit.add("symbolInsert", 0, b.rect);
            if b.clicked() || (enter && typed.is_some()) {
                asked = typed.clone();
            }
        });
        if let Some(params) = asked {
            match app.execute("insert.symbol", params) {
                Ok(_) => self.error.clear(),
                Err(e) => self.error = tl!(&e).to_string(),
            }
        }
        done
    }
}

#[cfg(test)]
#[path = "symbol_tests.rs"]
mod symbol_tests;
