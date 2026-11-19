// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pictures that come as attachments, shown as pictures in the message (#834).
//!
//! A photo sent from a phone comes as a file, not written into the body, and as a chip it says
//! nothing of what is in it. With the choice in File › Options (`ui.attachmentPictures`, kept in
//! `ui.json`, on unless the owner turns it off; classic Outlook shows chips), each
//! attachment that is a PNG, JPEG, GIF or WebP by its own bytes is a small picture, scaled down
//! to fit; a double-click opens it and its `▾` menu is the chip's. Everything else stays a chip.
//!
//! The bytes are hostile, as every attachment's: whether a file is a picture is told by its
//! bytes alone ([`images::probe`], within the reading pane's caps, never by its name or its
//! declared type), and it is decoded by [`images::decode`] with the same caps, off the window's
//! thread (one at a time), into a small texture. Mail in Junk E-mail is shown by names only. Only
//! the first [`MAX_PREVIEWS`] files of a message whose type or name says they may be pictures
//! are looked at, so that a message with a thousand attachments costs no more a frame than one
//! with a few. Nothing is fetched: a file that is kept on disk beside the mailbox is brought in
//! when its declared type or name says it may be a picture, at most [`MAX_PREVIEWS`] of a
//! message and [`MAX_BROUGHT_IN`] bytes, through the session's own memory budget, and only the
//! very files chosen: copies of one picture (one content, many attachments) are not brought in.

use std::sync::{Arc, Mutex, Weak};

use egui::{Rect, Ui, Vec2, pos2, vec2};
use mailcraft_model::{Attachment, ContentRef, Message};
use serde_json::json;

use super::{Place, images};
use crate::MailApp;
use crate::theme::{Tokens, paint_clipped, regular};

/// Most attachments of one message shown as pictures; the rest are chips.
pub const MAX_PREVIEWS: usize = 24;

/// Most bytes brought in from disk for the pictures of one message.
pub const MAX_BROUGHT_IN: u64 = 96 * 1024 * 1024;

/// Most pictures kept decoded; the least recently shown go first.
pub const MAX_THUMBS: usize = 64;

/// Most bytes compared by content in a frame to tell a picture read again from one that is new.
pub const MAX_COMPARED: usize = 4 * 1024 * 1024;

/// The room a picture is fitted into, in points: never wider or taller, never enlarged.
pub const BOX: Vec2 = Vec2::new(200.0, 140.0);

/// The narrowest a picture's tile is, so that its name has room.
const MIN_WIDTH: f32 = 120.0;

/// Room round the picture, and the height of the name and size under it.
const PAD: f32 = 6.0;
const CAPTION: f32 = 32.0;

/// Whether an attachment may be a picture that is shown, by what the message says of it: its
/// declared type, or for a file of no particular type its name. Only to choose which files kept
/// on disk are brought in: whether one is shown is told by its bytes.
pub fn may_be_picture(a: &Attachment) -> bool {
    let mime = a.mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match mime.as_str() {
        "image/png" | "image/x-png" | "image/jpeg" | "image/jpg" | "image/pjpeg" | "image/gif" | "image/webp" => true,
        "" | "application/octet-stream" => {
            let name = mailcraft_engine::opening::file_name(&a.name).to_ascii_lowercase();
            let ext = name.rsplit_once('.').map_or("", |(_, ext)| ext);
            matches!(ext, "png" | "jpg" | "jpeg" | "jpe" | "jfif" | "gif" | "webp")
        }
        _ => false,
    }
}

/// The files of `files` that are kept on disk and are to be brought in to be shown as pictures:
/// those that may be one, within the reading pane's cap on a picture's bytes, the first
/// [`MAX_PREVIEWS`] of them and no more than [`MAX_BROUGHT_IN`] bytes in all, counting those
/// already brought in.
pub fn to_bring_in(files: &[(usize, &Attachment)]) -> Vec<ContentRef> {
    let mut total = 0u64;
    let mut out = Vec::new();
    for (_, a) in files.iter().filter(|(_, a)| may_be_picture(a)).take(MAX_PREVIEWS) {
        let size = a.size() as u64;
        // What is in memory counts by its length, though a file brought in has no `kept` any more
        // (`Session::load_where`): the cap holds from one frame to the next.
        if a.is_loaded() {
            total = total.saturating_add(size);
            if total > MAX_BROUGHT_IN {
                break;
            }
            continue;
        }
        let Some(kept) = a.kept else { continue };
        if kept.len > images::MAX_PICTURE_BYTES as u64 {
            continue;
        }
        total = total.saturating_add(kept.len);
        if total > MAX_BROUGHT_IN {
            break;
        }
        out.push(kept);
    }
    out
}

/// What `Session::load_where` is given to bring in the files [`to_bring_in`] chose, `wanted`: only
/// those very files. One that has the content of a chosen file (a copy: one content, named by
/// many attachments) is not chosen for that, nor is one that can't be a picture or one after the
/// ones [`to_bring_in`] looked at. The files are asked about in order, as there, so that the
/// first `wanted.len()` that match are the ones it chose.
pub(super) fn chosen(wanted: &[ContentRef]) -> impl Fn(&Attachment) -> bool {
    let left = std::cell::Cell::new(wanted.len());
    move |a| {
        let it = left.get() > 0 && may_be_picture(a) && a.kept.is_some_and(|k| wanted.contains(&k));
        if it {
            left.set(left.get() - 1);
        }
        it
    }
}

/// Which of the files whose picture sizes are `sizes` (`None`: not a picture that is shown) are
/// shown as pictures: the first [`MAX_PREVIEWS`].
pub fn as_pictures(sizes: &[Option<[u32; 2]>]) -> Vec<Option<[u32; 2]>> {
    let mut left = MAX_PREVIEWS;
    sizes
        .iter()
        .map(|size| {
            let shown = size.filter(|_| left > 0);
            if shown.is_some() {
                left -= 1;
            }
            shown
        })
        .collect()
}

/// The size, in points, a picture of `size` pixels is shown at within `room`: scaled down to
/// fit, never up, at least a point each way.
pub fn fit(size: [u32; 2], room: Vec2) -> Vec2 {
    let [w, h] = size.map(|v| v.max(1) as f32);
    let scale = (room.x / w).min(room.y / h).min(1.0);
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    vec2((w * scale).round().max(1.0), (h * scale).round().max(1.0))
}

/// How the files of `m` are shown at `place`: for each, in order, its size in pixels when it is
/// shown as a picture, `None` for a chip. Those kept on disk that are to be shown as pictures are
/// brought in.
pub(super) fn looks(app: &mut MailApp, ctx: &egui::Context, m: &Message, place: Place, files: &[(usize, &Attachment)]) -> Vec<Option<[u32; 2]>> {
    let chips = || vec![None; files.len()];
    if !matches!(place, Place::Pane | Place::Window(_)) {
        return chips();
    }
    // Mail in Junk E-mail, or in a folder under it, is shown by names only: no picture is looked
    // at or brought in.
    if app.session.mailbox.in_junk(m.folder) || !app.ui.attachment_pictures {
        return chips();
    }
    let frame = images::frame(ctx);
    let thumbs = Thumbs::of(ctx);
    let mut thumbs = thumbs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    thumbs.pump(ctx);
    // Only the first files that may be pictures and are in memory are looked up: a message with
    // a thousand attachments costs no more a frame than one with [`MAX_PREVIEWS`].
    let mut asked = 0;
    let sizes: Vec<Option<[u32; 2]>> = files
        .iter()
        .map(|(_, a)| {
            if a.data.is_empty() || !may_be_picture(a) || asked >= MAX_PREVIEWS {
                return None;
            }
            asked += 1;
            thumbs.size(&a.data, frame)
        })
        .collect();
    let items = as_pictures(&sizes).into_iter().zip(files).map(|(size, (_, a))| size.filter(|_| !thumbs.failed(&a.data))).collect();
    drop(thumbs);
    bring_in(app, ctx, m, files);
    items
}

/// Bring in the files of `m` kept on disk that may be pictures ([`to_bring_in`]). One that can't
/// be read is not asked for again while the window is open: it stays a chip.
fn bring_in(app: &mut MailApp, ctx: &egui::Context, m: &Message, files: &[(usize, &Attachment)]) {
    if m.id == 0 {
        return;
    }
    let wanted = to_bring_in(files);
    if wanted.is_empty() {
        return;
    }
    // (Ids are reused when another mailbox is opened: the opening is part of the key.)
    let failed_key = egui::Id::unique(("mailcraft-attachment-pictures-failed", app.session.openings(), m.id));
    if ctx.data(|d| d.get_temp::<bool>(failed_key)).unwrap_or(false) {
        return;
    }
    match app.session.load_where(m.id, chosen(&wanted)) {
        // The message shown takes them up at the next frame.
        Ok(_) => ctx.request_repaint(),
        Err(_) => {
            log::warn!("message {}: its pictures couldn't be read", m.id);
            ctx.data_mut(|d| d.insert_temp(failed_key, true));
        }
    }
}

/// An attachment that is a picture of `size` pixels, as a small picture with its name and size
/// under it; its `▾` and what a double-click does are the chip's.
pub(super) fn tile(app: &mut MailApp, ui: &mut Ui, m: &Message, index: usize, a: &Attachment, size: [u32; 2], t: &Tokens) {
    let shown = mailcraft_engine::opening::file_name(&a.name);
    let picture = fit(size, BOX);
    let inner = picture.x.max(MIN_WIDTH);
    let tile = vec2(inner + 2.0 * PAD + crate::ribbon::DROP_W, PAD + BOX.y + CAPTION);
    // The texture is made for the size it is shown at, in pixels.
    let ppp = ui.ctx().pixels_per_point().clamp(1.0, 4.0);
    let max_side = u32::try_from(ui.ctx().input(|i| i.max_texture_side)).unwrap_or(images::MAX_TEXTURE_SIDE).min(images::MAX_TEXTURE_SIDE);
    let px = |points: f32| ((points * ppp).ceil() as u32).clamp(1, max_side);
    let wanted = [px(picture.x.max(picture.y)), px(picture.x)];
    let texture = {
        let thumbs = Thumbs::of(ui.ctx());
        let mut thumbs = thumbs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        thumbs.texture(ui.ctx(), &a.data, wanted)
    };
    let size_label = mailcraft_model::size_label_in(crate::i18n::current(), a.size() as u64);
    super::file_item(app, ui, m, index, &shown, tile, t, |painter, main| {
        let room = Rect::from_min_size(main.min + vec2(PAD, PAD), vec2(main.width() - 2.0 * PAD, BOX.y));
        let at = Rect::from_center_size(room.center(), picture);
        match &texture {
            Some(texture) => {
                let whole = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                painter.image(texture.id(), at, whole, egui::Color32::WHITE);
            }
            // Not decoded yet: its room, so that nothing moves when it comes.
            None => {
                painter.rect_filled(at, 2.0, t.hover);
                let mark = Rect::from_center_size(at.center(), Vec2::splat(20.0f32.min(at.width()).min(at.height())));
                crate::icons::paint(painter, mark, "picture", t.text_dim, t.accent);
            }
        }
        paint_clipped(painter, pos2(main.min.x + PAD, room.max.y + 10.0), inner, &shown, regular(12.0), t.text);
        painter.text(pos2(main.min.x + PAD, room.max.y + 24.0), egui::Align2::LEFT_CENTER, &size_label, regular(10.5), t.text_dim);
    });
}

/// File › Options: whether picture attachments are shown as pictures (`ui.attachmentPictures`).
pub(crate) fn options(app: &mut MailApp, ui: &mut Ui) {
    let mut on = app.ui.attachment_pictures;
    let response = ui
        .checkbox(&mut on, tl!("Show picture attachments as pictures"))
        .on_hover_text(tl!("Otherwise they are shown by their names. In Junk E-mail they are always shown by their names."));
    app.hit.add("attachmentPicturesOption", 0, response.rect);
    if response.changed() {
        let _ = app.run("ui.attachmentPictures", json!({"value": on}));
    }
}

/// What became of one picture.
enum State {
    /// Not decoded yet, nor being decoded.
    Waiting,
    /// Being decoded on a thread of its own.
    Decoding,
    Ready(egui::TextureHandle),
    /// It cannot be shown: it is a chip.
    Failed,
}

struct Thumb {
    /// Its bytes, as long as a message read has them.
    data: Weak<Vec<u8>>,
    /// Its size in pixels; `None` when it is not a picture that is shown.
    size: Option<[u32; 2]>,
    state: State,
    /// The frame it was last asked about.
    used: u64,
}

/// A picture being decoded off the window's thread.
#[cfg(not(target_arch = "wasm32"))]
struct Job {
    data: Weak<Vec<u8>>,
    receive: std::sync::mpsc::Receiver<Option<egui::ColorImage>>,
}

/// The pictures of attachments, decoded small.
pub struct Thumbs {
    entries: Vec<Thumb>,
    #[cfg(not(target_arch = "wasm32"))]
    job: Option<Job>,
    /// The frame in which a picture was last decoded on the window's thread.
    decoded_in: Option<u64>,
    /// The frame, and how many bytes were compared by content in it ([`MAX_COMPARED`]).
    compared: (u64, usize),
    /// Decode on the window's thread, one picture a frame, as the browser must: for tests, which
    /// want what they look at at once.
    pub inline: bool,
}

impl Default for Thumbs {
    fn default() -> Thumbs {
        Thumbs {
            entries: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            job: None,
            decoded_in: None,
            compared: (0, 0),
            inline: cfg!(test) || cfg!(target_arch = "wasm32"),
        }
    }
}

impl Thumbs {
    /// The window's.
    pub fn of(ctx: &egui::Context) -> Arc<Mutex<Thumbs>> {
        ctx.data_mut(|d| d.get_temp_mut_or_default::<Arc<Mutex<Thumbs>>>(egui::Id::unique("mailcraft-attachment-pictures")).clone())
    }

    /// The entry of the picture `data`, made when there is none and room for one.
    fn entry(&mut self, data: &Arc<Vec<u8>>, frame: u64) -> Option<&mut Thumb> {
        // What no message read has any more can't be asked for again by these bytes.
        self.entries.retain(|e| e.data.strong_count() > 0);
        let index = match self.entries.iter().position(|e| std::ptr::eq(e.data.as_ptr(), Arc::as_ptr(data))) {
            Some(index) => index,
            // The same picture as new bytes (a message read again is read anew): its entry holds
            // these from now on.
            None => match self.same_bytes(data, frame) {
                Some(index) => {
                    let entry = self.entries.get_mut(index)?;
                    entry.data = Arc::downgrade(data);
                    index
                }
                None => {
                    if self.entries.len() >= MAX_THUMBS {
                        let oldest = self.entries.iter().enumerate().filter(|(_, e)| e.used < frame).min_by_key(|(_, e)| e.used).map(|(i, _)| i);
                        // Everything kept is on screen now: this one is a chip.
                        drop(self.entries.swap_remove(oldest?));
                    }
                    let size = images::probe(data);
                    self.entries.push(Thumb { data: Arc::downgrade(data), size, state: State::Waiting, used: frame });
                    self.entries.len() - 1
                }
            },
        };
        let entry = self.entries.get_mut(index)?;
        entry.used = frame;
        Some(entry)
    }

    /// The entry whose bytes are equal to `data`, by content. The work is bounded: no more than
    /// [`MAX_COMPARED`] bytes are compared in a frame, after which the bytes are taken as new.
    fn same_bytes(&mut self, data: &Arc<Vec<u8>>, frame: u64) -> Option<usize> {
        if self.compared.0 != frame {
            self.compared = (frame, 0);
        }
        for (index, e) in self.entries.iter().enumerate() {
            // (Not one being decoded: its thread finds it by the bytes it was given.)
            if matches!(e.state, State::Decoding) {
                continue;
            }
            let Some(held) = e.data.upgrade().filter(|d| d.len() == data.len()) else { continue };
            if self.compared.1.saturating_add(data.len()) > MAX_COMPARED {
                return None;
            }
            self.compared.1 += data.len();
            if held == *data {
                return Some(index);
            }
        }
        None
    }

    /// The size in pixels of the picture `data`; `None` when it is not one that is shown.
    pub fn size(&mut self, data: &Arc<Vec<u8>>, frame: u64) -> Option<[u32; 2]> {
        self.entry(data, frame).and_then(|e| e.size)
    }

    /// Whether the picture `data` was tried and couldn't be decoded.
    pub fn failed(&self, data: &Arc<Vec<u8>>) -> bool {
        self.entries.iter().any(|e| std::ptr::eq(e.data.as_ptr(), Arc::as_ptr(data)) && matches!(e.state, State::Failed))
    }

    /// The texture of the picture `data`, decoded within `[max_side, max_width]` pixels; `None`
    /// until it is decoded. Its decoding is started when no other is under way.
    pub fn texture(&mut self, ctx: &egui::Context, data: &Arc<Vec<u8>>, [max_side, max_width]: [u32; 2]) -> Option<egui::TextureHandle> {
        let frame = images::frame(ctx);
        let inline = self.inline;
        let busy = self.busy(frame);
        let decoded_here = self.decoded_in == Some(frame);
        let entry = self.entry(data, frame)?;
        match &entry.state {
            State::Ready(texture) => return Some(texture.clone()),
            State::Decoding | State::Failed => return None,
            State::Waiting if entry.size.is_none() => {
                entry.state = State::Failed;
                return None;
            }
            State::Waiting if busy => {
                // A picture was decoded on the window's thread in this frame: the next frame
                // takes this one. (A thread at work asks for one itself when it is done.)
                if decoded_here {
                    ctx.request_repaint();
                }
                return None;
            }
            State::Waiting => {}
        }
        if !inline && self.spawn(ctx, data, max_side, max_width) {
            return None;
        }
        // Here: in the browser, in a test, or when no thread could be started.
        self.decoded_in = Some(frame);
        let image = images::decode(data, max_side, max_width);
        let entry = self.entry(data, frame)?;
        entry.state = match image {
            Some(image) => State::Ready(ctx.load_texture("mailcraft-attachment-picture", image, egui::TextureOptions::LINEAR)),
            None => {
                // A header that was fine and data that is not: the placeholder drawn in this
                // frame gives way to the chip at the next one, which is asked for.
                ctx.request_repaint();
                State::Failed
            }
        };
        match &entry.state {
            State::Ready(texture) => Some(texture.clone()),
            _ => None,
        }
    }

    /// Whether no picture may start decoding now: one is under way, or one was decoded on the
    /// window's thread in this frame.
    fn busy(&self, frame: u64) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if self.job.is_some() {
            return true;
        }
        self.decoded_in == Some(frame)
    }

    /// Start decoding `data` on a thread of its own; whether it was started.
    #[cfg(not(target_arch = "wasm32"))]
    fn spawn(&mut self, ctx: &egui::Context, data: &Arc<Vec<u8>>, max_side: u32, max_width: u32) -> bool {
        let (send, receive) = std::sync::mpsc::channel();
        let bytes = data.clone();
        let repaint = ctx.clone();
        let job = std::thread::Builder::new().name("attachment-picture".into()).spawn(move || {
            let _ = send.send(images::decode(&bytes, max_side, max_width));
            repaint.request_repaint();
        });
        match job {
            Ok(_) => {
                self.job = Some(Job { data: Arc::downgrade(data), receive });
                if let Some(entry) = self.entries.iter_mut().find(|e| std::ptr::eq(e.data.as_ptr(), Arc::as_ptr(data))) {
                    entry.state = State::Decoding;
                }
                true
            }
            Err(e) => {
                log::warn!("couldn't start decoding a picture: {e}");
                false
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn spawn(&mut self, _: &egui::Context, _: &Arc<Vec<u8>>, _: u32, _: u32) -> bool {
        false
    }

    /// Take in the picture the thread decoded, if it is done.
    pub fn pump(&mut self, ctx: &egui::Context) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(job) = &self.job else { return };
            let image = match job.receive.try_recv() {
                Ok(image) => image,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => None,
            };
            let at = job.data.as_ptr();
            self.job = None;
            // (One let go of meanwhile is not kept.)
            if let Some(entry) = self.entries.iter_mut().find(|e| std::ptr::eq(e.data.as_ptr(), at) && e.data.strong_count() > 0) {
                entry.state = match image {
                    Some(image) => State::Ready(ctx.load_texture("mailcraft-attachment-picture", image, egui::TextureOptions::LINEAR)),
                    None => State::Failed,
                };
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = ctx;
    }

    /// How many bytes were compared by content in `frame`.
    #[cfg(test)]
    pub fn compared_in(&self, frame: u64) -> usize {
        if self.compared.0 == frame { self.compared.1 } else { 0 }
    }

    /// How many pictures are kept, and how many of them are decoded.
    #[cfg(test)]
    pub fn counts(&self) -> (usize, usize) {
        (self.entries.len(), self.entries.iter().filter(|e| matches!(e.state, State::Ready(_))).count())
    }
}
