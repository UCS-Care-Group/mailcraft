// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The font an account writes in (File › Signatures): Outlook's Font dialog, for one of the three
//! fonts of one account. It only collects the choice and runs `signature.fonts`.
//!
//! What it offers is what a message can be written in here — a family, a size, bold, italic,
//! underline and a colour. Outlook's effects (strike through, hidden, all caps) have no
//! equivalent in what MailCraft writes, so the dialog shows no box for them rather than a box
//! that does nothing.

use egui::{RichText, Ui, vec2};
use mailcraft_model::rich::{Built, Font, MAX_SIZE, MIN_SIZE, safe_color};
use mailcraft_model::{AccountKind, FontSlot};
use serde_json::{Map, Value, json};

use crate::MailApp;
use crate::dialogs::buttons;
use crate::fonts::{self, Pick};
use crate::ribbon::COLORS;
use crate::theme::{Tokens, mono, regular, semibold};

pub const WIDTH: f32 = 360.0;

/// How many installed families the list shows at once; what is left is reached by typing.
const MOST_FONTS: usize = 400;

/// The font for one account's one slot, as the dialog holds it while it is being chosen.
pub struct FontDialog {
    pub account: u64,
    pub slot: FontSlot,
    /// The family as `format.font` takes it — a name, or a whole list. Empty is the message's own.
    family: String,
    bold: bool,
    italic: bool,
    underline: bool,
    /// Points, or none for the message's own.
    size: Option<u8>,
    /// `"#rrggbb"`, or none for automatic.
    color: Option<String>,
    /// What is being typed in the font list's search field.
    search: String,
    /// Why OK was refused, until something is changed.
    error: String,
}

impl FontDialog {
    /// The dialog for one account's font: `account` (default: the default account) and `slot`
    /// (`new`, `reply` or `plain`), starting from what that account chose. `None` when there is no
    /// such account — or no such slot, which is nobody's font to choose.
    pub fn open(app: &MailApp, params: &Value) -> Option<FontDialog> {
        let slot = FontSlot::parse(params.get("slot").and_then(Value::as_str)?)?;
        let account =
            params.get("account").and_then(Value::as_u64).filter(|i| *i != 0).or_else(|| app.session.mailbox.default_account().map(|a| a.id))?;
        let a = app.session.mailbox.account(account)?;
        // Its settings are written back as its file had them; a choice here would be lost.
        if a.kind == AccountKind::Unknown {
            return None;
        }
        let now = a.default_font(slot);
        Some(FontDialog {
            account,
            slot,
            family: now.and_then(|s| s.font.as_ref()).map(|f| f.list().to_string()).unwrap_or_default(),
            bold: now.is_some_and(|s| s.bold),
            italic: now.is_some_and(|s| s.italic),
            underline: now.is_some_and(|s| s.underline),
            size: now.and_then(|s| s.size),
            color: now.and_then(|s| s.color.clone()),
            search: String::new(),
            error: String::new(),
        })
    }

    /// The window's title: what it is, and whose. Which of the three the font is, the first line
    /// inside says — there is no room for both in the title.
    pub fn title(&self, app: &MailApp) -> String {
        let whose = app.session.mailbox.account(self.account).map(|a| a.email.clone()).unwrap_or_default();
        crate::i18n::fmt(tl!("Font — {account}"), &[("account", &whose)])
    }

    /// Which of the three this is, in words.
    fn slot_line(&self) -> String {
        match self.slot {
            FontSlot::New => tl!("The font new messages are written in").to_string(),
            FontSlot::Reply => tl!("The font replies and forwards are written in").to_string(),
            FontSlot::Plain => tl!("The font plain-text messages are written and read in").to_string(),
        }
    }

    /// The command's parameters: the whole font, or null for the message's own.
    fn params(&self) -> Value {
        let mut font = Map::new();
        let family = self.family.trim();
        if !family.is_empty() {
            font.insert("font".into(), json!(family));
        }
        if let Some(n) = self.size {
            font.insert("size".into(), json!(n));
        }
        for (key, on) in [("bold", self.bold), ("italic", self.italic), ("underline", self.underline)] {
            if on {
                font.insert(key.into(), json!(true));
            }
        }
        if let Some(hex) = &self.color {
            font.insert("color".into(), json!(hex));
        }
        json!({"account": self.account, self.slot.key(): if font.is_empty() { Value::Null } else { Value::Object(font) }})
    }

    /// The family as the box shows it: the first name of the list, in its own spelling.
    fn shown_family(&self) -> String {
        if self.family.is_empty() {
            return tl!("(the message's own)").to_string();
        }
        Font::parse(&self.family).map(|f| f.label().to_string()).unwrap_or_else(|| self.family.clone())
    }

    /// The style as one word, as Outlook's dialog lists them.
    fn shown_style(&self) -> String {
        match (self.bold, self.italic) {
            (true, true) => tl!("Bold Italic"),
            (true, false) => tl!("Bold"),
            (false, true) => tl!("Italic"),
            _ => tl!("Regular"),
        }
        .to_string()
    }

    /// The font list: the message's own, the six families MailCraft knows, and the ones installed
    /// here — the same list the ribbon's Font button gives, searched the same way (#1493).
    fn family_combo(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let t = Tokens::get(&ctx);
        let now = Font::parse(&self.family);
        let key = now.as_ref().map(|f| f.key().to_string());
        let built = key.as_deref().and_then(Built::from_key);
        let mut chosen: Option<String> = None;
        egui::ComboBox::from_id_salt(("font-dialog-family", self.account, self.slot.key())).selected_text(self.shown_family()).width(220.0).show_ui(
            ui,
            |ui| {
                if ui.selectable_label(self.family.is_empty(), tl!("(the message's own)")).clicked() {
                    chosen = Some(String::new());
                    return;
                }
                for b in Built::ALL {
                    if ui.selectable_label(built == Some(b), b.label()).clicked() {
                        chosen = Some(b.list().to_string());
                        return;
                    }
                }
                ui.separator();
                let mut query = self.search.clone();
                let field = crate::text_menu::field(
                    ui.add(egui::TextEdit::singleline(&mut query).hint_text(tl!("Find a font")).char_limit(64).desired_width(196.0)),
                );
                crate::text_menu::hold(&field);
                self.search = query;
                match fonts::offered(&ctx, &self.search, MOST_FONTS) {
                    None => {
                        ui.label(RichText::new(tl!("Looking for fonts…")).color(t.text_dim));
                    }
                    Some((names, cut)) => {
                        egui::ScrollArea::vertical().id_salt("font-dialog-list").max_height(200.0).auto_shrink([false, true]).show(ui, |ui| {
                            for name in &names {
                                let on = key.as_deref() == Some(name.to_lowercase().as_str());
                                if ui.selectable_label(on, name.as_str()).clicked() {
                                    // The family as it is named here, with a font of its kind after
                                    // it for whoever doesn't have it, as the ribbon's list gives it.
                                    chosen = Some(format!("{}, {}", name, fonts::generic_family(&ctx, name)));
                                    return;
                                }
                            }
                            if cut > 0 {
                                ui.label(
                                    RichText::new(tln!(cut, "…and {n} more — type to narrow it", "…and {n} more — type to narrow it"))
                                        .color(t.text_dim),
                                );
                            }
                        });
                    }
                }
            },
        );
        if let Some(family) = chosen {
            self.family = family;
            self.error.clear();
        }
    }

    /// The colour box: automatic, the palette, and a value of the owner's own — the colours the
    /// ribbon's Font Color button offers.
    fn colour_combo(&mut self, ui: &mut Ui) {
        let shown = match &self.color {
            None => tl!("Automatic").to_string(),
            Some(hex) => hex.clone(),
        };
        let mut chosen: Option<Option<String>> = None;
        egui::ComboBox::from_id_salt(("font-dialog-color", self.account, self.slot.key())).selected_text(shown).width(220.0).show_ui(ui, |ui| {
            if ui.selectable_label(self.color.is_none(), tl!("Automatic")).clicked() {
                chosen = Some(None);
            }
            ui.horizontal_wrapped(|ui| {
                ui.set_max_width(200.0);
                for (name, hex) in COLORS {
                    let Ok(c) = egui::Color32::from_hex(hex) else { continue };
                    let on = self.color.as_deref() == Some(hex);
                    let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::click());
                    ui.painter().rect_filled(r, 2.0, c);
                    if on || resp.hovered() {
                        ui.painter().rect_stroke(r.expand(1.5), 2.0, egui::Stroke::new(1.5, ui.visuals().text_color()), egui::StrokeKind::Outside);
                    }
                    if resp.on_hover_text(tl!(name)).clicked() {
                        chosen = Some(Some((*hex).to_string()));
                    }
                }
            });
            ui.separator();
            let mut typed = self.color.clone().unwrap_or_default();
            let field =
                crate::text_menu::field(ui.add(egui::TextEdit::singleline(&mut typed).hint_text("#rrggbb").char_limit(7).desired_width(90.0)));
            crate::text_menu::hold(&field);
            if ui.add_enabled(!typed.trim().is_empty(), egui::Button::new(tl!("Set"))).clicked() {
                chosen = Some(Some(typed.trim().to_string()));
            }
        });
        if let Some(color) = chosen {
            // A colour that isn't one is said here, where it was typed, rather than refused by OK.
            match &color {
                Some(hex) if safe_color(hex).is_none() => self.error = tl!("The colour is \"#rrggbb\", or Automatic").to_string(),
                _ => {
                    self.color = color;
                    self.error.clear();
                }
            }
        }
    }

    /// The sample: the alphabet as the message would be written in what is chosen so far.
    fn sample(&self, ui: &mut Ui) {
        let t = Tokens::get(ui.ctx());
        let size: f32 = self.size.unwrap_or(12).into();
        let mut chooser = fonts::Chooser::new(ui.ctx());
        let pick = if self.family.is_empty() { Pick::Own } else { chooser.pick(&self.family, self.bold) };
        let font = match pick {
            Pick::Mono => mono(size),
            Pick::Family(family) => egui::FontId::new(size, family),
            Pick::Own if self.bold => semibold(size),
            Pick::Own => regular(size),
        };
        chooser.finish(ui.ctx());
        let color = self.color.as_deref().and_then(|h| egui::Color32::from_hex(h).ok()).unwrap_or(t.text);
        egui::Frame::new().fill(t.input).stroke(egui::Stroke::new(1.0, t.border)).corner_radius(4.0).inner_margin(12.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(tl!("Sample"));
                let sample = RichText::new("AaBbCcYyZz").font(font).color(color);
                ui.label(if self.underline { sample.underline() } else { sample });
            });
        });
    }

    /// Draws the dialog; whether it is over.
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> bool {
        let t = Tokens::get(ui.ctx());
        // The list is made on a thread of its own the first time it is asked for; the window is
        // asked to draw again when it arrives.
        fonts::ask(ui.ctx());
        ui.label(RichText::new(self.slot_line()).font(regular(11.5)).color(t.text_dim));
        ui.add_space(8.0);
        egui::Grid::new("font-dialog").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
            ui.label(tl!("Font"));
            self.family_combo(ui);
            ui.end_row();
            ui.label(tl!("Font style"));
            let styles = [tl!("Regular").to_string(), tl!("Bold").to_string(), tl!("Italic").to_string(), tl!("Bold Italic").to_string()];
            let now = usize::from(self.italic) + 2 * usize::from(self.bold);
            let mut pick: Option<usize> = None;
            egui::ComboBox::from_id_salt(("font-dialog-style", self.account, self.slot.key()))
                .selected_text(self.shown_style())
                .width(220.0)
                .show_ui(ui, |ui| {
                    for (i, name) in styles.iter().enumerate() {
                        if ui.selectable_label(i == now, name.as_str()).clicked() {
                            pick = Some(i);
                        }
                    }
                });
            if let Some(i) = pick {
                self.bold = i & 2 != 0;
                self.italic = i & 1 != 0;
            }
            ui.end_row();
            ui.label(tl!("Size"));
            let mut size: Option<Option<u8>> = None;
            egui::ComboBox::from_id_salt(("font-dialog-size", self.account, self.slot.key()))
                .selected_text(self.size.map_or_else(|| tl!("(the message's own)").to_string(), |n| n.to_string()))
                .width(220.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(self.size.is_none(), tl!("(the message's own)")).clicked() {
                        size = Some(None);
                    }
                    for n in MIN_SIZE..=MAX_SIZE {
                        if ui.selectable_label(self.size == Some(n), n.to_string()).clicked() {
                            size = Some(Some(n));
                        }
                    }
                });
            if let Some(n) = size {
                self.size = n;
            }
            ui.end_row();
            ui.label(tl!("Font color"));
            self.colour_combo(ui);
            ui.end_row();
        });
        ui.add_space(2.0);
        ui.checkbox(&mut self.underline, tl!("Underline"));
        ui.add_space(8.0);
        self.sample(ui);
        if !self.error.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new(self.error.as_str()).color(t.red));
        }
        let (ok, cancel) = buttons(ui, tl!("OK"), true);
        if ok || enter {
            match app.execute("signature.fonts", self.params()) {
                Ok(_) => return true,
                Err(e) => self.error = tl!(&e).to_string(),
            }
        }
        cancel
    }
}

#[cfg(test)]
#[path = "font_dialog_tests.rs"]
mod font_dialog_tests;
