// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! On-demand preview of an attachment in the message's reading pane.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc::Receiver};

use mailcraft_engine::Part;
use mailcraft_model::{Attachment, Id};
use serde_json::{Value, json};

use crate::MailApp;

pub const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PAGES: u32 = 10_000;
pub const MAX_SIDE: u32 = 3_072;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Pdf,
    Png,
    Jpeg,
    Tiff,
    Webp,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Tiff => "tif",
            Self::Webp => "webp",
        }
    }

    fn hint(part: &Attachment) -> Option<Self> {
        let mime = part.mime.to_ascii_lowercase();
        match mime.as_str() {
            "application/pdf" => return Some(Self::Pdf),
            "image/png" => return Some(Self::Png),
            "image/jpeg" => return Some(Self::Jpeg),
            "image/tiff" => return Some(Self::Tiff),
            "image/webp" => return Some(Self::Webp),
            _ => {}
        }
        match part.name.rsplit_once('.')?.1.to_ascii_lowercase().as_str() {
            "pdf" => Some(Self::Pdf),
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "tif" | "tiff" => Some(Self::Tiff),
            "webp" => Some(Self::Webp),
            _ => None,
        }
    }

    pub fn matches(self, data: &[u8]) -> bool {
        match self {
            Self::Pdf => data.starts_with(b"%PDF-"),
            Self::Png => data.starts_with(b"\x89PNG\r\n\x1a\n"),
            Self::Jpeg => data.starts_with(b"\xff\xd8\xff"),
            Self::Tiff => data.starts_with(b"II*\0") || data.starts_with(b"MM\0*"),
            Self::Webp => data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    Invalid,
    TooLarge,
    TimedOut,
    Busy,
    Failed,
}

impl Error {
    fn text(self) -> &'static str {
        match self {
            Self::Unavailable => tl!("No preview helper is available for this attachment."),
            Self::Invalid => tl!("This attachment could not be previewed."),
            Self::TooLarge => tl!("This attachment is too large to preview."),
            Self::TimedOut => tl!("The attachment preview timed out."),
            Self::Busy => tl!("Wait for the other attachment preview to finish."),
            Self::Failed => tl!("The attachment preview could not be opened."),
        }
    }
}

pub struct Request {
    pub bytes: Arc<Vec<u8>>,
    pub format: Format,
    pub page: u32,
    pub pages: Option<u32>,
    pub cancel: Arc<AtomicBool>,
}

pub struct Page {
    pub image: egui::ColorImage,
    pub pages: u32,
}

pub struct Host {
    pub pdf: bool,
    pub images: bool,
    pub render: Box<dyn Fn(Request) -> Receiver<Result<Page, Error>>>,
}

impl Host {
    fn supports(&self, format: Format) -> bool {
        if format == Format::Pdf { self.pdf } else { self.images }
    }
}

pub(crate) struct Preview {
    message: Id,
    index: usize,
    name: String,
    viewport: egui::ViewportId,
    mailbox: u64,
    bytes: Arc<Vec<u8>>,
    format: Format,
    page: u32,
    pages: Option<u32>,
    pending: Option<Receiver<Result<Page, Error>>>,
    cancel: Arc<AtomicBool>,
    texture: Option<egui::TextureHandle>,
    error: Option<Error>,
    zoom: f32,
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub(crate) fn available(app: &MailApp, id: Id, index: usize) -> bool {
    app.session
        .mailbox
        .message(id)
        .and_then(|m| m.attachments.get(index))
        .and_then(Format::hint)
        .is_some_and(|format| app.services.attachment_preview.as_ref().is_some_and(|host| host.supports(format)))
}

pub(crate) fn selected(app: &MailApp, viewport: egui::ViewportId, id: Id, index: usize) -> bool {
    app.attachment_preview.as_ref().is_some_and(|p| p.viewport == viewport && p.message == id && p.index == index)
}

pub(crate) fn open(app: &mut MailApp, viewport: egui::ViewportId, id: Id, index: usize) -> Result<(), String> {
    let host = app.services.attachment_preview.as_ref().ok_or_else(|| Error::Unavailable.text())?;
    let part = app.session.mailbox.message(id).and_then(|m| m.attachments.get(index)).ok_or_else(|| Error::Invalid.text())?;
    let format = Format::hint(part).filter(|format| host.supports(*format)).ok_or_else(|| Error::Unavailable.text())?;
    if part.size() > MAX_INPUT_BYTES {
        return Err(Error::TooLarge.text().into());
    }
    if selected(app, viewport, id, index) && app.attachment_preview.as_ref().is_some_and(|preview| preview.error.is_none()) {
        return Ok(());
    }
    let name = mailcraft_engine::opening::file_name(&part.name);
    let bytes = app.session.content(id, Part::Attachment(index)).map_err(|_| Error::Invalid.text())?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(Error::TooLarge.text().into());
    }
    if !format.matches(&bytes) {
        return Err(Error::Invalid.text().into());
    }
    // Dropping a preview tells the worker to stop before the next render starts.
    app.attachment_preview = None;
    let mut preview = Preview {
        message: id,
        index,
        name,
        viewport,
        mailbox: app.session.openings(),
        bytes,
        format,
        page: 1,
        pages: None,
        pending: None,
        cancel: Arc::new(AtomicBool::new(false)),
        texture: None,
        error: None,
        zoom: 1.0,
    };
    preview.start(host, 1);
    app.attachment_preview = Some(preview);
    Ok(())
}

pub(crate) fn command(app: &mut MailApp, params: &Value) -> Result<Value, String> {
    let id = params.get("id").and_then(Value::as_u64).or_else(|| app.session.sel.focus()).ok_or_else(|| Error::Invalid.text())?;
    let index = params.get("index").and_then(Value::as_u64).and_then(|i| usize::try_from(i).ok()).ok_or_else(|| Error::Invalid.text())?;
    if app.session.sel.focus() != Some(id) || app.session.sel.messages.len() != 1 {
        return Err(tl!("Select the message before previewing its attachment.").into());
    }
    open(app, egui::ViewportId::ROOT, id, index)?;
    Ok(json!({"preview": true}))
}

impl Preview {
    fn start(&mut self, host: &Host, page: u32) {
        self.page = page;
        self.texture = None;
        self.error = None;
        self.pending =
            Some((host.render)(Request { bytes: self.bytes.clone(), format: self.format, page, pages: self.pages, cancel: self.cancel.clone() }));
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.pending else { return };
        let result = match pending.try_recv() {
            Ok(value) => value,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(Error::Failed),
        };
        self.pending = None;
        match result {
            Ok(page) if (self.page..=MAX_PAGES).contains(&page.pages) => {
                self.pages = Some(page.pages);
                self.texture = Some(ctx.load_texture("attachment-preview", page.image, egui::TextureOptions::LINEAR));
            }
            Ok(_) => self.error = Some(Error::Invalid),
            Err(error) => self.error = Some(error),
        }
    }
}

pub(crate) fn follow_message(app: &mut MailApp, viewport: egui::ViewportId, id: Option<Id>) {
    if app.attachment_preview.as_ref().is_some_and(|p| p.viewport == viewport && Some(p.message) != id) {
        app.attachment_preview = None;
    }
}

pub(crate) fn poll(app: &mut MailApp, ctx: &egui::Context) {
    if let Some(preview) = &mut app.attachment_preview {
        if preview.mailbox == app.session.openings() {
            preview.poll(ctx);
        } else {
            app.attachment_preview = None;
        }
    }
}

pub(crate) fn show(app: &mut MailApp, ui: &mut egui::Ui, id: Id) -> bool {
    let viewport = ui.ctx().viewport_id();
    let Some(preview) = app.attachment_preview.as_mut().filter(|p| p.viewport == viewport && p.message == id) else { return false };
    let tokens = crate::theme::Tokens::get(ui.ctx());
    let mut turn = None;
    let mut close = false;
    egui::Frame::NONE.fill(tokens.panel).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
        ui.horizontal_wrapped(|ui| {
            let back = ui.button(tl!("Back to Message"));
            app.hit.add("attachmentPreviewBack", id, back.rect);
            close = back.clicked();
            ui.add(egui::Label::new(egui::RichText::new(&preview.name).strong()).truncate());
        });
        ui.horizontal_wrapped(|ui| {
            if let Some(pages) = preview.pages.filter(|pages| *pages > 1) {
                let ready = preview.pending.is_none();
                if ui.add_enabled(ready && preview.page > 1, egui::Button::new(tl!("Previous Page"))).clicked() {
                    turn = Some(preview.page - 1);
                }
                ui.label(format!("{} / {pages}", preview.page));
                let next = ui.add_enabled(ready && preview.page < pages, egui::Button::new(tl!("Next Page")));
                app.hit.add("attachmentPreviewNext", id, next.rect);
                if next.clicked() {
                    turn = Some(preview.page + 1);
                }
            }
            if preview.texture.is_some() {
                ui.add(egui::Slider::new(&mut preview.zoom, 1.0..=3.0).text(tl!("Zoom")));
            }
        });
    });
    if close {
        app.attachment_preview = None;
        return false;
    }
    if let (Some(page), Some(host)) = (turn, &app.services.attachment_preview) {
        preview.start(host, page);
    }
    ui.separator();
    if let Some(texture) = &preview.texture {
        let size = texture.size_vec2();
        let room = ui.available_size().max(egui::vec2(1.0, 1.0));
        let display = size * (room.x / size.x).min(room.y / size.y).min(1.0) * preview.zoom;
        egui::Frame::NONE.fill(tokens.canvas).show(ui, |ui| {
            egui::ScrollArea::both().auto_shrink([false, false]).id_salt(("attachment-preview", id, preview.index)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(((room.x - display.x) / 2.0).max(0.0));
                    let image = ui.add(egui::Image::new((texture.id(), display)));
                    app.hit.add("attachmentPreviewImage", id, image.rect);
                });
            });
        });
    } else {
        egui::Frame::NONE.inner_margin(12).show(ui, |ui| {
            if let Some(error) = preview.error {
                ui.colored_label(tokens.red, error.text());
            } else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tl!("Preparing attachment preview…"));
                });
            }
        });
    }
    true
}
