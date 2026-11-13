// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Print (#32): the Print dialog, and what the window hands the system. The engine lays the
//! messages out as a PDF (`Session::print_pdf`) with the header's words in the interface language;
//! the dialog only collects the printer and how to print, and the host sends the PDF to the
//! printer ([`Host`]). Without a host that prints, the PDF is opened in the system's PDF viewer to
//! be printed from there, or downloaded in the browser.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use egui::{RichText, Ui, vec2};
use mailcraft_engine::print::{Labels, Request};
use mailcraft_model::Id;
use mailcraft_print::{Doc, Fonts, Options, Paper};
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

/// Why the host printed nothing, said in the interface language ([`PrintError::said`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrintError {
    /// The printer's name isn't one that can be printed to.
    Printer,
    /// The pages asked for aren't a list of page numbers.
    Pages,
    /// The file to print couldn't be written or named; what the system said.
    File(String),
    /// The printer refused the job.
    Refused,
    /// The printer didn't answer in time: the job may still print.
    NoAnswer,
}

impl PrintError {
    pub fn said(&self) -> String {
        match self {
            PrintError::Printer => tl!("That printer can't be printed to.").to_string(),
            PrintError::Pages => tl!("The pages to print aren't a list of page numbers.").to_string(),
            PrintError::File(e) => crate::i18n::fmt(tl!("The file to print couldn't be written: {error}"), &[("error", e)]),
            PrintError::Refused => tl!("The printer didn't take the job. Check that it is on and connected, or Save as PDF instead.").to_string(),
            PrintError::NoAnswer => {
                tl!("The printer didn't answer in time. The job may still print: look at the print queue before printing again.").to_string()
            }
        }
    }
}

/// Desktop: what the system offers for printing. Both are called on a thread of their own, never
/// the window's: each may wait for the system.
pub struct Host {
    /// The printers, the default first; empty when there are none or they can't be listed.
    pub printers: Arc<dyn Fn() -> Vec<Printer> + Send + Sync>,
    /// Send a job to its printer. An error says why nothing was printed.
    pub submit: Arc<dyn Fn(Job) -> Result<(), PrintError> + Send + Sync>,
}

/// `work` done on a thread of its own, its answer to be picked up from the receiver; in the
/// browser, which has no threads, and when no thread can be had, done at once.
fn background<T: Send + 'static>(name: &str, work: impl FnOnce() -> T + Send + 'static) -> Receiver<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let work = std::sync::Arc::new(Mutex::new(Some(work)));
        let shared = work.clone();
        let sender = tx.clone();
        let spawned = std::thread::Builder::new().name(name.to_string()).spawn(move || {
            if let Some(work) = shared.lock().ok().and_then(|mut w| w.take()) {
                let _ = sender.send(work());
            }
        });
        if spawned.is_ok() {
            return rx;
        }
        if let Some(work) = work.lock().ok().and_then(|mut w| w.take()) {
            let _ = tx.send(work());
        }
        rx
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = name;
        let _ = tx.send(work());
        rx
    }
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

/// A print laid out for one paper: its bytes and page count, or why it couldn't be.
type Laid = Result<(Arc<Vec<u8>>, usize), String>;

/// Where a print for one paper is.
enum Layout {
    Busy(Receiver<Laid>),
    Done(Laid),
}

/// The Print dialog. What it prints is taken when it opens (the messages, laid out by the engine
/// as documents), so that a change of the selection meanwhile, by a sync or an agent, prints
/// nothing else than what the dialog counted. The pages are laid out on a thread of their own,
/// once for each paper, and the printers are listed and the job is sent there too: the window
/// never waits for the system.
pub struct PrintDialog {
    ids: Vec<Id>,
    docs: Arc<Vec<Doc>>,
    fonts: Fonts,
    options: Options,
    /// The PDF's name.
    name: String,
    printers: Vec<Printer>,
    listing: Option<Receiver<Vec<Printer>>>,
    printer: usize,
    copies: u32,
    all_pages: bool,
    range: String,
    letter: bool,
    two_sided: bool,
    color: bool,
    /// The print on A4 and on Letter, as far as they have been laid out.
    laid: [Option<Layout>; 2],
    /// A job on its way to the printer, and what to say when it is there.
    sending: Option<(Receiver<Result<(), PrintError>>, String)>,
    error: String,
}

impl PrintDialog {
    /// The dialog for the messages `params` names, or the selected ones; `None` when there is
    /// nothing to print (the command then says why).
    pub fn open(app: &MailApp, params: &Value) -> Option<PrintDialog> {
        let mut ids: Vec<Id> = params.get("ids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
        ids.extend(params.get("id").and_then(Value::as_u64));
        let ids = app.session.print_ids(&ids).ok()?;
        let request = Request { ids: ids.clone(), labels: labels(), paper: None };
        let docs = app.session.print_docs(&request).ok()?;
        let fonts = app.session.print_fonts().ok()?;
        let options = app.session.print_options(&docs, &request);
        let name = mailcraft_engine::cmd::file::pdf_name(&app.session, &ids);
        let listing = app.services.print.as_ref().map(|h| {
            let printers = h.printers.clone();
            background("printers", move || printers())
        });
        Some(PrintDialog {
            ids,
            docs: Arc::new(docs),
            fonts,
            options,
            name,
            printers: Vec::new(),
            listing,
            printer: 0,
            copies: 1,
            all_pages: true,
            range: String::new(),
            letter: app.session.print_paper() == Paper::LETTER,
            two_sided: false,
            color: true,
            laid: [None, None],
            sending: None,
            error: String::new(),
        })
    }

    /// The messages it prints, as it took them.
    pub fn ids(&self) -> &[Id] {
        &self.ids
    }

    /// Whether something is still being worked out: the pages, the printers, a job on its way.
    pub fn busy(&self) -> bool {
        self.listing.is_some() || self.sending.is_some() || matches!(self.laid.get(usize::from(self.letter)), Some(Some(Layout::Busy(_))))
    }

    /// The print for the paper chosen: laid out, started, or picked up when it is done.
    fn current(&mut self) -> Option<&Laid> {
        let slot = self.laid.get_mut(usize::from(self.letter))?;
        if slot.is_none() {
            let (docs, fonts) = (self.docs.clone(), self.fonts.clone());
            let options = Options { paper: if self.letter { Paper::LETTER } else { Paper::A4 }, ..self.options.clone() };
            let rx = background("print-layout", move || {
                mailcraft_engine::print::render(&docs, &fonts, &options).map(|(bytes, pages)| (Arc::new(bytes), pages))
            });
            *slot = Some(Layout::Busy(rx));
        }
        if let Some(Layout::Busy(rx)) = slot {
            match rx.try_recv() {
                Ok(laid) => *slot = Some(Layout::Done(laid)),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    *slot = Some(Layout::Done(Err(tl!("The messages couldn't be laid out for printing.").to_string())))
                }
            }
        }
        match slot {
            Some(Layout::Done(laid)) => Some(laid),
            _ => None,
        }
    }

    /// What has come back from the threads: the printers, a job sent.
    fn pick_up(&mut self, app: &mut MailApp) -> bool {
        if let Some(rx) = &self.listing {
            match rx.try_recv() {
                Ok(printers) => {
                    self.printer = printers.iter().position(|p| p.default).unwrap_or(0);
                    self.printers = printers;
                    self.listing = None;
                }
                Err(TryRecvError::Disconnected) => self.listing = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        let Some((rx, said)) = &self.sending else { return false };
        let answer = match rx.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Err(PrintError::Refused),
        };
        let said = said.clone();
        self.sending = None;
        match answer {
            Ok(()) => {
                app.status(said);
                true
            }
            Err(e) => {
                self.error = e.said();
                false
            }
        }
    }

    /// The dialog's contents; whether it is done.
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> bool {
        let t = Tokens::get(ui.ctx());
        let mut done = self.pick_up(app);
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
            } else if self.listing.is_some() {
                ui.label(RichText::new(tl!("Looking for printers…")).color(t.text_dim));
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
        // Read after the paper is chosen: a change of it in this frame never prints the other's.
        let laid = self.current().cloned();
        if self.busy() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
        }
        ui.add_space(2.0);
        match &laid {
            None => {
                ui.label(RichText::new(tl!("Laying out the pages…")).color(t.text_dim));
            }
            Some(Ok((_, pages))) => {
                ui.label(RichText::new(tln!(*pages, "{n} page", "{n} pages")).color(t.text_dim));
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e.as_str()).color(t.red));
            }
        }
        if self.sending.is_some() {
            ui.label(RichText::new(tl!("Sending to the printer…")).color(t.text_dim));
        }
        if !self.error.is_empty() {
            ui.label(RichText::new(self.error.as_str()).color(t.red));
        }
        let range_ok = self.all_pages || valid_range(&self.range);
        let ready = matches!(laid, Some(Ok(_))) && self.sending.is_none() && self.listing.is_none();
        ui.add_space(4.0);
        button_row(ui, |ui| {
            let b = ui.add(egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0)));
            app.hit.add("printCancel", 0, b.rect);
            done |= b.clicked();
            let b = primary_button(ui, tl!("Print"), range_ok && ready);
            app.hit.add("printGo", 0, b.rect);
            if (b.clicked() || enter)
                && range_ok
                && ready
                && let Some(Ok((pdf, pages))) = &laid
            {
                self.error.clear();
                match self.print(app, pdf, *pages) {
                    Ok(Some(said)) => {
                        app.status(said);
                        done = true;
                    }
                    Ok(None) => {}
                    Err(e) => self.error = e,
                }
            }
        });
        done
    }

    /// Send the print to the printer, on a thread of its own (`None`: what to say comes when it
    /// is there), or open it in the viewer (what to say).
    fn print(&mut self, app: &mut MailApp, pdf: &Arc<Vec<u8>>, pages: usize) -> Result<Option<String>, String> {
        let name = self.name.clone();
        let printer = self.printers.get(self.printer).cloned();
        match (&app.services.print, printer) {
            (Some(host), Some(printer)) => {
                let job = Job {
                    printer: printer.name.clone(),
                    title: name.trim_end_matches(".pdf").to_string(),
                    pdf: pdf.as_ref().clone(),
                    copies: self.copies.clamp(1, MAX_COPIES),
                    pages: (!self.all_pages).then(|| self.range.split_whitespace().collect::<String>()),
                    two_sided: self.two_sided,
                    color: self.color,
                    paper: if self.letter { "Letter" } else { "A4" },
                };
                let said = crate::i18n::fmt(
                    &tln!(pages, "Printing {n} page on {printer}", "Printing {n} pages on {printer}"),
                    &[("printer", &printer.label)],
                );
                let submit = host.submit.clone();
                self.sending = Some((background("print-job", move || submit(job)), said));
                Ok(None)
            }
            _ => {
                if let Some(open_file) = &app.services.open_file {
                    open_file(&name, pdf)?;
                    return Ok(Some(crate::i18n::fmt(tl!("Opened {name} to print"), &[("name", &name)])));
                }
                if let Some(download) = &app.services.download {
                    download(&name, pdf)?;
                    return Ok(Some(crate::i18n::fmt(tl!("Downloaded {name}: open it to print"), &[("name", &name)])));
                }
                Err(tl!("Printing isn't available here: use Save as PDF instead.").to_string())
            }
        }
    }
}
