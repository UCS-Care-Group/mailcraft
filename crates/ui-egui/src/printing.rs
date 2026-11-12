// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Print (#32): the Print dialog, and what the window hands the system. The engine lays the
//! messages out as a PDF (`Session::print_pdf`) with the header's words in the interface language;
//! the dialog only collects the printer and how to print, and the host sends the PDF to the
//! printer ([`Host`]). Without a host that prints, the PDF is opened in the system's PDF viewer to
//! be printed from there, or downloaded in the browser.

use egui::{RichText, Ui, vec2};
use mailcraft_engine::print::{Labels, Request};
use mailcraft_model::Id;
use mailcraft_print::Paper;
use serde_json::{Value, json};

use crate::MailApp;
use crate::dialogs::button_row;
use crate::theme::Tokens;
use crate::widgets::primary_button;

pub const WIDTH: f32 = 420.0;
/// The most copies the dialog asks for.
pub const MAX_COPIES: u32 = 99;
/// The longest page range the dialog takes.
const MAX_RANGE: usize = 64;

/// A printer the system knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Printer {
    /// The name the system prints to it by.
    pub name: String,
    /// The name to show, when the system has a friendlier one.
    pub label: String,
    /// The system's default printer.
    pub default: bool,
}

/// One thing to print.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub printer: String,
    pub title: String,
    pub pdf: Vec<u8>,
    pub copies: u32,
    /// The pages, as `1-3,5`; `None` for all.
    pub pages: Option<String>,
    pub two_sided: bool,
    pub color: bool,
    /// `A4` or `Letter`.
    pub paper: &'static str,
}

/// Desktop: what the system offers for printing.
pub struct Host {
    /// The printers, the default first; empty when there are none or they can't be listed.
    pub printers: Box<dyn Fn() -> Vec<Printer>>,
    /// Send a job to its printer. An error says why nothing was printed.
    pub submit: Box<dyn Fn(Job) -> Result<(), String>>,
}

/// The header's words in the interface language, for the engine.
pub fn labels() -> Labels {
    Labels {
        from: tl!("From:").into(),
        sent: tl!("Sent:").into(),
        to: tl!("To:").into(),
        cc: tl!("Cc:").into(),
        attachments: tl!("Attachments:").into(),
        page: tl!("Page {n} of {m}").into(),
    }
}

/// `params` with the header's words in the interface language, when the command prints and the
/// caller named none.
pub fn with_labels(id: &str, params: Value) -> Value {
    if !matches!(id, "file.print" | "file.saveAsPdf") {
        return params;
    }
    let mut map = match params {
        Value::Object(map) => map,
        Value::Null => serde_json::Map::new(),
        other => return other,
    };
    if !map.contains_key("labels") {
        let l = labels();
        map.insert("labels".into(), json!({"from": l.from, "sent": l.sent, "to": l.to, "cc": l.cc, "attachments": l.attachments, "page": l.page}));
    }
    Value::Object(map)
}

/// Whether `text` is a page range the printer takes: numbers and ranges of them, with commas.
pub fn valid_range(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || text.len() > MAX_RANGE {
        return false;
    }
    text.split(',').all(|part| {
        let part = part.trim();
        let mut ends = part.splitn(2, '-').map(str::trim);
        let first = ends.next().and_then(|n| n.parse::<u32>().ok()).filter(|n| *n > 0);
        match (first, ends.next()) {
            (Some(_), None) => true,
            (Some(a), Some(b)) => b.parse::<u32>().ok().is_some_and(|b| b >= a),
            _ => false,
        }
    })
}

/// The Print dialog.
pub struct PrintDialog {
    ids: Vec<Id>,
    printers: Vec<Printer>,
    printer: usize,
    copies: u32,
    all_pages: bool,
    range: String,
    letter: bool,
    two_sided: bool,
    color: bool,
    /// The page count for the paper it was counted on.
    pages: Option<(bool, usize)>,
    error: String,
}

impl PrintDialog {
    /// The dialog for the messages `params` names, or the selected ones; `None` when there is
    /// nothing to print (the command then says why).
    pub fn open(app: &MailApp, params: &Value) -> Option<PrintDialog> {
        let mut ids: Vec<Id> = params.get("ids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
        ids.extend(params.get("id").and_then(Value::as_u64));
        let request = Request { ids: ids.clone(), labels: labels(), paper: None };
        app.session.print_docs(&request).ok()?;
        let printers = app.services.print.as_ref().map(|h| (h.printers)()).unwrap_or_default();
        let printer = printers.iter().position(|p| p.default).unwrap_or(0);
        Some(PrintDialog {
            ids,
            printers,
            printer,
            copies: 1,
            all_pages: true,
            range: String::new(),
            letter: app.session.print_paper() == Paper::LETTER,
            two_sided: false,
            color: true,
            pages: None,
            error: String::new(),
        })
    }

    fn request(&self) -> Request {
        Request { ids: self.ids.clone(), labels: labels(), paper: Some(if self.letter { Paper::LETTER } else { Paper::A4 }) }
    }

    /// The dialog's contents; whether it is done.
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> bool {
        let t = Tokens::get(ui.ctx());
        let mut done = false;
        if self.pages.is_none_or(|(letter, _)| letter != self.letter) {
            self.pages = app.session.print_pdf(&self.request()).ok().map(|(_, pages, _)| (self.letter, pages));
        }
        let can_print = app.services.print.is_some() && !self.printers.is_empty();
        egui::Grid::new("printDialog").num_columns(2).spacing(vec2(16.0, 8.0)).show(ui, |ui| {
            ui.label(tl!("Printer"));
            if can_print {
                let shown = self.printers.get(self.printer).map(|p| p.label.clone()).unwrap_or_default();
                let resp = egui::ComboBox::from_id_salt("printPrinter").width(260.0).selected_text(shown).show_ui(ui, |ui| {
                    for (i, p) in self.printers.iter().enumerate() {
                        ui.selectable_value(&mut self.printer, i, p.label.as_str());
                    }
                });
                app.hit.add("printPrinter", 0, resp.response.rect);
            } else {
                ui.label(RichText::new(tl!("Opens in your PDF viewer, to print from there")).color(t.text_dim));
            }
            ui.end_row();
            if can_print {
                ui.label(tl!("Copies"));
                let resp = ui.add(egui::DragValue::new(&mut self.copies).range(1..=MAX_COPIES));
                app.hit.add("printCopies", 0, resp.rect);
                ui.end_row();
                ui.label(tl!("Pages"));
                ui.horizontal(|ui| {
                    let all = ui.radio_value(&mut self.all_pages, true, tl!("All"));
                    app.hit.add("printAllPages", 0, all.rect);
                    let some = ui.radio_value(&mut self.all_pages, false, tl!("Pages:"));
                    app.hit.add("printSomePages", 0, some.rect);
                    let field = ui.add_enabled(
                        !self.all_pages,
                        egui::TextEdit::singleline(&mut self.range).hint_text("1-3, 5").char_limit(MAX_RANGE).desired_width(90.0),
                    );
                    app.hit.add("printRange", 0, field.rect);
                });
                ui.end_row();
            }
            ui.label(tl!("Paper"));
            ui.horizontal(|ui| {
                let a4 = ui.radio_value(&mut self.letter, false, "A4");
                app.hit.add("printA4", 0, a4.rect);
                let letter = ui.radio_value(&mut self.letter, true, tl!("Letter"));
                app.hit.add("printLetter", 0, letter.rect);
            });
            ui.end_row();
            if can_print {
                ui.label("");
                ui.vertical(|ui| {
                    let b = ui.checkbox(&mut self.two_sided, tl!("Print on both sides"));
                    app.hit.add("printTwoSided", 0, b.rect);
                    let c = ui.checkbox(&mut self.color, tl!("Print in colour"));
                    app.hit.add("printColor", 0, c.rect);
                });
                ui.end_row();
            }
        });
        if let Some((_, pages)) = self.pages {
            ui.add_space(2.0);
            let text = tln!(pages, "{n} page", "{n} pages");
            ui.label(RichText::new(text).color(t.text_dim));
        }
        if !self.error.is_empty() {
            ui.label(RichText::new(self.error.as_str()).color(t.red));
        }
        let range_ok = self.all_pages || valid_range(&self.range);
        ui.add_space(4.0);
        button_row(ui, |ui| {
            let b = ui.add(egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0)));
            app.hit.add("printCancel", 0, b.rect);
            done |= b.clicked();
            let b = primary_button(ui, tl!("Print"), range_ok);
            app.hit.add("printGo", 0, b.rect);
            if (b.clicked() || enter) && range_ok {
                match self.print(app) {
                    Ok(said) => {
                        app.status(said);
                        done = true;
                    }
                    Err(e) => self.error = e,
                }
            }
        });
        done
    }

    /// Lay the messages out and send them to the printer (or the viewer); what to say.
    fn print(&self, app: &mut MailApp) -> Result<String, String> {
        let (pdf, pages, _) = app.session.print_pdf(&self.request()).map_err(|e| e.to_string())?;
        let name = mailcraft_engine::cmd::file::pdf_name(&app.session, &self.ids);
        let printer = self.printers.get(self.printer).filter(|_| !self.printers.is_empty());
        match (&app.services.print, printer) {
            (Some(host), Some(printer)) => {
                let job = Job {
                    printer: printer.name.clone(),
                    title: name.trim_end_matches(".pdf").to_string(),
                    pdf,
                    copies: self.copies.clamp(1, MAX_COPIES),
                    pages: (!self.all_pages).then(|| self.range.split_whitespace().collect::<String>()),
                    two_sided: self.two_sided,
                    color: self.color,
                    paper: if self.letter { "Letter" } else { "A4" },
                };
                (host.submit)(job)?;
                Ok(crate::i18n::fmt(
                    &tln!(pages, "Printing {n} page on {printer}", "Printing {n} pages on {printer}"),
                    &[("printer", &printer.label)],
                ))
            }
            _ => {
                if let Some(open_file) = &app.services.open_file {
                    open_file(&name, &pdf)?;
                    return Ok(crate::i18n::fmt(tl!("Opened {name} to print"), &[("name", &name)]));
                }
                if let Some(download) = &app.services.download {
                    download(&name, &pdf)?;
                    return Ok(crate::i18n::fmt(tl!("Downloaded {name}: open it to print"), &[("name", &name)]));
                }
                Err(tl!("Printing isn't available here: use Save as PDF instead.").to_string())
            }
        }
    }
}
