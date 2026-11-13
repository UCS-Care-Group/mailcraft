// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! On-demand preview of an attachment in the message's reading pane.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc::Receiver};
use std::time::Duration;

use mailcraft_model::{Attachment, Id};
use serde_json::{Value, json};

use crate::MailApp;

pub const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PAGES: u32 = 10_000;
pub const MAX_SIDE: u32 = 3_072;
const RETRY_DELAY: Duration = Duration::from_millis(25);
const MAX_RETRIES: u8 = 120;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Pdf,
    Png,
    Jpeg,
    Tiff,
    Webp,
}

impl Format {
    /// The fixed extension used for the temporary input name.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Tiff => "tif",
            Self::Webp => "webp",
        }
    }

    /// Identify a possible format from metadata before loading the bytes.
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

    /// Check the file signature before handing bytes to the helper.
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
    /// Explain a refused or failed preview in the interface's language.
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

type ReadAttachment = Arc<dyn Fn() -> Result<Arc<Vec<u8>>, Error> + Send + Sync>;

pub struct Request {
    read: ReadAttachment,
    pub format: Format,
    pub page: u32,
    pub pages: Option<u32>,
    pub cancel: Arc<AtomicBool>,
}

impl Request {
    /// Load and validate the attachment on the renderer's worker thread.
    pub fn load(&self) -> Result<Arc<Vec<u8>>, Error> {
        let bytes = (self.read)()?;
        if bytes.len() > MAX_INPUT_BYTES {
            return Err(Error::TooLarge);
        }
        if !self.format.matches(&bytes) {
            return Err(Error::Invalid);
        }
        Ok(bytes)
    }
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
    /// Whether a helper was found for this format.
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
    attachment: Attachment,
    read: ReadAttachment,
    format: Format,
    page: u32,
    pages: Option<u32>,
    pending: Option<Receiver<Result<Page, Error>>>,
    retries: u8,
    retry_at: Option<f64>,
    cancel: Arc<AtomicBool>,
    texture: Option<egui::TextureHandle>,
    error: Option<Error>,
    zoom: f32,
}

pub(crate) struct Click {
    message: Id,
    index: usize,
    viewport: egui::ViewportId,
    mailbox: u64,
    attachment: Attachment,
    at: f64,
}

impl Drop for Preview {
    /// Stop a renderer when its preview is replaced or closed.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Whether this attachment can be offered in the explicit preview menu.
pub(crate) fn available(app: &MailApp, id: Id, index: usize) -> bool {
    app.ui.attachment_previews
        && app
            .session
            .mailbox
            .message(id)
            .and_then(|m| m.attachments.get(index))
            .filter(|part| (1..=MAX_INPUT_BYTES).contains(&part.size()))
            .and_then(Format::hint)
            .is_some_and(|format| app.services.attachment_preview.as_ref().is_some_and(|host| host.supports(format)))
}

/// Show the opt-in control for the helpers this installation found.
pub(crate) fn options(app: &mut MailApp, ui: &mut egui::Ui) {
    let Some(host) = &app.services.attachment_preview else { return };
    let label = match (host.pdf, host.images) {
        (true, true) => tl!("Preview attachments with local PdfCraft and PhotoCraft"),
        (true, false) => tl!("Preview attachments with local PdfCraft"),
        (false, true) => tl!("Preview attachments with local PhotoCraft"),
        (false, false) => return,
    };
    let response = ui
        .checkbox(&mut app.ui.attachment_previews, label)
        .on_hover_text(tl!("The local program receives the selected attachment and runs with your permissions."));
    app.hit.add("attachmentPreviewOption", 0, response.rect);
    if response.changed() && !app.ui.attachment_previews {
        app.attachment_preview = None;
        app.pending_attachment_preview = None;
    }
}

/// Whether a chip click can preview the attachment without opening its menu.
pub(crate) fn single_click_allowed(app: &MailApp, id: Id, index: usize) -> bool {
    available(app, id, index) && !in_junk(app, id)
}

/// Apply the mailbox's Junk policy to the message, including descendants of Junk.
fn in_junk(app: &MailApp, id: Id) -> bool {
    app.session.mailbox.message(id).is_some_and(|m| app.session.mailbox.in_junk(m.folder))
}

/// Defer the chip action until a double-click can be ruled out.
pub(crate) fn queue_click(app: &mut MailApp, ctx: &egui::Context, id: Id, index: usize) {
    let Some(attachment) = attachment(app, id, index).cloned() else { return };
    let delay = ctx.options(|o| o.input_options.max_double_click_delay).clamp(0.0, 2.0);
    let delay = if delay.is_finite() { delay } else { 0.3 };
    app.pending_attachment_preview = Some(Click {
        message: id,
        index,
        viewport: ctx.viewport_id(),
        mailbox: app.session.openings(),
        attachment,
        at: ctx.input(|i| i.time) + delay,
    });
    crate::software::repaint_after(ctx, Duration::from_secs_f64(delay + 0.001));
}

/// Discard the queued single-click action without starting a helper.
pub(crate) fn cancel_click(app: &mut MailApp) {
    app.pending_attachment_preview = None;
}

/// Start the queued action after checking that it still names the same attachment.
pub(crate) fn finish_click(app: &mut MailApp, ctx: &egui::Context, id: Id) {
    let Some(click) = app.pending_attachment_preview.as_ref().filter(|click| click.viewport == ctx.viewport_id() && click.message == id) else {
        return;
    };
    let remaining = click.at - ctx.input(|i| i.time);
    if remaining >= 0.0 {
        crate::software::repaint_after(ctx, Duration::from_secs_f64(remaining + 0.001));
        return;
    }
    let Some(click) = app.pending_attachment_preview.take() else { return };
    if single_click_allowed(app, click.message, click.index)
        && attachment(app, click.message, click.index) == Some(&click.attachment)
        && let Err(error) = open(app, click.viewport, click.message, click.index)
    {
        app.status(error);
    }
}

/// Look up a filed attachment without loading its contents.
fn attachment(app: &MailApp, id: Id, index: usize) -> Option<&Attachment> {
    app.session.mailbox.message(id)?.attachments.get(index)
}

/// Whether this chip owns the preview currently shown in the viewport.
pub(crate) fn selected(app: &MailApp, viewport: egui::ViewportId, id: Id, index: usize) -> bool {
    app.attachment_preview.as_ref().is_some_and(|p| p.viewport == viewport && p.message == id && p.index == index)
}

/// Replace any old preview and submit the selected attachment's first page.
pub(crate) fn open(app: &mut MailApp, viewport: egui::ViewportId, id: Id, index: usize) -> Result<(), String> {
    cancel_click(app);
    if !app.ui.attachment_previews {
        return Err(tl!("Turn on attachment previews in File › Options.").into());
    }
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
    if !part.data.is_empty() && !format.matches(&part.data) {
        return Err(Error::Invalid.text().into());
    }
    let read = app.session.attachment_reader(part).map_err(|_| Error::Invalid.text())?;
    let attachment = part.clone();
    // Dropping a preview tells the worker to stop before the next render starts.
    app.attachment_preview = None;
    let mut preview = Preview {
        message: id,
        index,
        name,
        viewport,
        mailbox: app.session.openings(),
        attachment,
        read: Arc::new(move || read().map_err(|_| Error::Invalid)),
        format,
        page: 1,
        pages: None,
        pending: None,
        retries: 0,
        retry_at: None,
        cancel: Arc::new(AtomicBool::new(false)),
        texture: None,
        error: None,
        zoom: 1.0,
    };
    preview.start(host, 1);
    app.attachment_preview = Some(preview);
    Ok(())
}

/// Handle an owner's script request, which still requires an explicit menu in Junk.
pub(crate) fn command(app: &mut MailApp, params: &Value) -> Result<Value, String> {
    let id = params.get("id").and_then(Value::as_u64).or_else(|| app.session.sel.focus()).ok_or_else(|| Error::Invalid.text())?;
    let index = params.get("index").and_then(Value::as_u64).and_then(|i| usize::try_from(i).ok()).ok_or_else(|| Error::Invalid.text())?;
    if app.session.sel.focus() != Some(id) || app.session.sel.messages.len() != 1 {
        return Err(tl!("Select the message before previewing its attachment.").into());
    }
    if in_junk(app, id) {
        return Err(tl!("In Junk E-mail, preview an attachment from its menu.").into());
    }
    open(app, egui::ViewportId::ROOT, id, index)?;
    Ok(json!({"preview": true}))
}

impl Preview {
    /// Start a page with its own retry budget.
    fn start(&mut self, host: &Host, page: u32) {
        self.page = page;
        self.texture = None;
        self.error = None;
        self.retries = 0;
        self.retry_at = None;
        self.submit(host);
    }

    /// Submit the current page to the optional desktop host.
    fn submit(&mut self, host: &Host) {
        self.pending = Some((host.render)(Request {
            read: self.read.clone(),
            format: self.format,
            page: self.page,
            pages: self.pages,
            cancel: self.cancel.clone(),
        }));
    }

    /// Pick up a worker result or retry after a superseded render releases the slot.
    fn poll(&mut self, ctx: &egui::Context, host: &Host) {
        let now = ctx.input(|i| i.time);
        if let Some(at) = self.retry_at {
            if now < at {
                ctx.request_repaint_after_for(Duration::from_secs_f64(at - now), self.viewport);
                return;
            }
            self.retry_at = None;
            self.submit(host);
        }
        let Some(pending) = &self.pending else { return };
        let result = match pending.try_recv() {
            Ok(value) => value,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(Error::Failed),
        };
        ctx.request_repaint_of(self.viewport);
        self.pending = None;
        match result {
            Ok(page) if (self.page..=MAX_PAGES).contains(&page.pages) => {
                self.pages = Some(page.pages);
                self.texture = Some(ctx.load_texture("attachment-preview", page.image, egui::TextureOptions::LINEAR));
            }
            Ok(_) => self.error = Some(Error::Invalid),
            Err(Error::Busy) if self.retries < MAX_RETRIES => {
                self.retries += 1;
                self.retry_at = Some(now + RETRY_DELAY.as_secs_f64());
                ctx.request_repaint_after_for(RETRY_DELAY, self.viewport);
            }
            Err(error) => self.error = Some(error),
        }
    }
}

/// Release a preview or click when its viewport starts showing another message.
pub(crate) fn follow_message(app: &mut MailApp, viewport: egui::ViewportId, id: Option<Id>) {
    if app.attachment_preview.as_ref().is_some_and(|p| p.viewport == viewport && Some(p.message) != id) {
        app.attachment_preview = None;
    }
    if app.pending_attachment_preview.as_ref().is_some_and(|click| click.viewport == viewport && Some(click.message) != id) {
        cancel_click(app);
    }
}

/// Drop stale attachment state before polling the active preview.
pub(crate) fn poll(app: &mut MailApp, ctx: &egui::Context) {
    if app.pending_attachment_preview.is_none() && app.attachment_preview.is_none() {
        return;
    }
    let viewports = crate::windows::viewports(app);
    let window_is_open = |viewport| viewport == egui::ViewportId::ROOT || viewports.contains(&viewport);
    if app.pending_attachment_preview.as_ref().is_some_and(|click| click.mailbox != app.session.openings() || !window_is_open(click.viewport)) {
        cancel_click(app);
    }
    if app.attachment_preview.as_ref().is_some_and(|preview| {
        preview.mailbox != app.session.openings()
            || !window_is_open(preview.viewport)
            || attachment(app, preview.message, preview.index) != Some(&preview.attachment)
    }) {
        app.attachment_preview = None;
    }
    if let (Some(preview), Some(host)) = (&mut app.attachment_preview, &app.services.attachment_preview) {
        preview.poll(ctx, host);
    }
}

/// Draw the preview in place of the message body when it belongs to this viewport.
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
                let ready = preview.pending.is_none() && preview.retry_at.is_none();
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
                    crate::software::spinner(ui, ui.spacing().interact_size.y, None);
                    ui.label(tl!("Preparing attachment preview…"));
                });
            }
        });
    }
    true
}
