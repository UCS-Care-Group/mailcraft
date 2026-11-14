// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The fonts a message asks for (#600): its text is drawn in the first font of its `font-family`
//! that is installed on this computer, and nothing is ever fetched for it.
//!
//! - A name is looked up in the fonts installed here ([`FontIndex`], made once, on a thread of its
//!   own, the first time a message names a font), in the order the message gives them; the first
//!   that is installed wins, and a face that can't be read counts as not installed.
//! - `serif` is the first of this system's serif fonts that is installed ([`SERIF`]); `monospace`
//!   is the window's own monospaced font; `sans-serif`, `system-ui`, `cursive`, `fantasy` and the
//!   other generic families are the window's own font, and so is a list none of whose fonts is
//!   installed.
//! - Bold text is drawn in the family's bold face; a family without one draws it in the window's
//!   bold, as the composer does (#582): egui can't make a face bold itself.
//! - A face is read whole only when a message is to be drawn in it, on a thread of its own, and
//!   then kept for the life of the program: at most [`MAX_FACES`] faces and [`MAX_BYTES`] bytes,
//!   each at most [`MAX_FACE_BYTES`]; past those, a font counts as not installed.
//! - The faces of the composer's fonts, read at start (#582), are used as they are.
//!
//! A face is drawn with only once egui has taken it in, the pass after it was given (naming a
//! family egui doesn't have would stop the program): until then the text is laid out in the
//! window's font, and laid out again when the fonts change ([`generation`]). The browser has no
//! fonts folder: there `monospace` is all that changes anything.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use egui::{FontDefinitions, FontFamily};
use mailcraft_model::rich::Font;

use crate::font_index::FontIndex;

/// The most faces read for messages, and the most bytes of them together; and the largest face.
pub const MAX_FACES: usize = 32;
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// (A collection of Chinese, Japanese and Korean faces is about twenty megabytes.)
pub const MAX_FACE_BYTES: u64 = 32 * 1024 * 1024;

/// How many faces one reading thread reads.
const BATCH: usize = 8;

/// How many lists of fonts a layout remembers the answer for.
const REMEMBERED: usize = 256;

/// The names the families of message fonts begin with, among egui's.
const PREFIX: &str = "message-font:";

/// The serif fonts a message's `serif` is drawn in, best first: the first that is installed.
#[cfg(windows)]
pub const SERIF: &[&str] = &["times new roman", "georgia", "cambria"];
#[cfg(target_os = "macos")]
pub const SERIF: &[&str] = &["times", "times new roman", "new york", "georgia"];
#[cfg(all(not(windows), not(target_os = "macos")))]
pub const SERIF: &[&str] = &["dejavu serif", "liberation serif", "noto serif", "tinos", "freeserif", "times new roman"];

/// What a span's fonts come to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    /// The window's own font (regular, or its bold for bold text).
    Own,
    /// The window's monospaced font.
    Mono,
    /// An installed face, registered under this family.
    Family(FontFamily),
}

/// The family a message font's face is registered under in egui.
pub fn family_of(name: &str, bold: bool) -> FontFamily {
    FontFamily::Name(format!("{PREFIX}{name}{}", if bold { ":bold" } else { "" }).into())
}

enum Generic {
    Serif,
    Mono,
    Own,
}

/// The generic families, and the keywords that stand for one.
fn generic(name: &str) -> Option<Generic> {
    Some(match name {
        "serif" | "ui-serif" => Generic::Serif,
        "monospace" | "ui-monospace" => Generic::Mono,
        "sans-serif" | "ui-sans-serif" | "system-ui" | "-apple-system" | "blinkmacsystemfont" | "cursive" | "fantasy" | "emoji" | "math"
        | "fangsong" | "ui-rounded" => Generic::Own,
        _ => return None,
    })
}

/// What is known of a name's face.
enum Lookup {
    Found(Pick),
    /// Not installed (or unreadable): the next name of the list is tried.
    Absent,
    /// Installed, or perhaps installed, and not read yet: the window's font meanwhile.
    Coming,
}

/// What a face read for messages has come to.
enum FaceState {
    /// Waiting to be read, or being read.
    Queued,
    Ready(Arc<egui::FontData>),
    /// Not to be had: not installed after all, unreadable, too big, or past the caps.
    Missing,
}

#[derive(Default)]
enum IndexState {
    #[default]
    NotAsked,
    #[cfg(not(target_arch = "wasm32"))]
    Building(std::sync::mpsc::Receiver<FontIndex>),
    Ready(Arc<FontIndex>),
}

/// A face as it is read: which file and which face of it, and its data when it could be read.
type FaceFile = (Arc<std::path::Path>, u32);
type Read = Vec<((String, bool), Option<(FaceFile, Arc<egui::FontData>)>)>;

/// The message fonts of a window: the index of what is installed and the faces read.
#[derive(Default)]
pub struct Store {
    index: IndexState,
    faces: HashMap<(String, bool), FaceState>,
    queue: Vec<(String, bool)>,
    /// What the reading thread has in hand, and how it is to come back.
    reading: Vec<(String, bool)>,
    #[cfg(not(target_arch = "wasm32"))]
    loading: Option<std::sync::mpsc::Receiver<Read>>,
    /// The faces read, by file: one face is read once, whatever names it is asked for by.
    files: HashMap<FaceFile, Arc<egui::FontData>>,
    /// Faces read, and their bytes.
    read: usize,
    bytes: u64,
    /// How many faces are ready, and how many could not be had (a list that named one goes on to
    /// its next font).
    ready: usize,
    missing: usize,
    /// Faces that are ready and may not be in egui's fonts yet.
    dirty: bool,
    /// The work is done on the window's thread, at once: for tests.
    inline: bool,
}

impl Store {
    fn of(ctx: &egui::Context) -> Arc<Mutex<Store>> {
        ctx.data_mut(|d| d.get_temp_mut_or_default::<Arc<Mutex<Store>>>(egui::Id::unique("mailcraft-message-fonts")).clone())
    }

    /// Use `index` for what is installed and do every read on the window's thread: for tests.
    #[cfg(test)]
    pub fn use_index(ctx: &egui::Context, index: FontIndex) {
        let store = Store::of(ctx);
        let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
        *store = Store { index: IndexState::Ready(Arc::new(index)), inline: true, ..Store::default() };
    }

    /// As though `read` faces of `bytes` bytes had been read already: for tests of the caps.
    #[cfg(test)]
    pub fn pretend_read(ctx: &egui::Context, read: usize, bytes: u64) {
        let store = Store::of(ctx);
        let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
        (store.read, store.bytes) = (read, bytes);
    }

    /// The faces read for messages, and their bytes: for tests.
    #[cfg(test)]
    pub fn counts(ctx: &egui::Context) -> (usize, u64) {
        let store = Store::of(ctx);
        let store = store.lock().unwrap_or_else(|e| e.into_inner());
        (store.read, store.bytes)
    }

    fn index(&self) -> Option<Arc<FontIndex>> {
        match &self.index {
            IndexState::Ready(index) => Some(index.clone()),
            _ => None,
        }
    }

    /// Whether the installed fonts may yet be known: on a system with fonts folders, until the
    /// index is made. (Not in tests, which lay out in the same fonts on every computer unless
    /// they give fonts of their own, [`Store::use_index`].)
    fn index_coming(&self) -> bool {
        !cfg!(any(target_arch = "wasm32", test)) && !matches!(self.index, IndexState::Ready(_))
    }

    /// Whether no more faces may be read.
    fn full(&self) -> bool {
        self.read >= MAX_FACES || self.bytes >= MAX_BYTES
    }

    fn set(&mut self, key: (String, bool), state: FaceState) {
        match &state {
            FaceState::Ready(_) => {
                self.ready = self.ready.saturating_add(1);
                self.dirty = true;
            }
            FaceState::Missing => self.missing = self.missing.saturating_add(1),
            FaceState::Queued => {}
        }
        self.faces.insert(key, state);
    }

    /// A face a layout wanted: read already under another name, past the caps, or queued.
    fn want(&mut self, key: (String, bool)) {
        if self.faces.contains_key(&key) {
            return;
        }
        let file = self.index().and_then(|index| index.face(&key.0, key.1).map(|f| (f.path.clone(), f.index)));
        if let Some(data) = file.as_ref().and_then(|file| self.files.get(file)).cloned() {
            self.set(key, FaceState::Ready(data));
        } else if file.is_none() || self.full() || self.queue.len() >= MAX_FACES {
            // At once, with no thread: a message naming many fonts costs nothing more.
            self.set(key, FaceState::Missing);
        } else {
            self.faces.insert(key.clone(), FaceState::Queued);
            self.queue.push(key);
        }
    }

    /// Start making the index, once.
    fn ask_index(&mut self, ctx: &egui::Context) {
        if !matches!(self.index, IndexState::NotAsked) {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            if self.inline {
                self.index = IndexState::Ready(Arc::new(FontIndex::system()));
                return;
            }
            let (send, receive) = std::sync::mpsc::channel();
            let ctx = ctx.clone();
            let job = std::thread::Builder::new().name("font-index".into()).spawn(move || {
                let index = FontIndex::system();
                log::debug!("fonts installed: {} families", index.len());
                let _ = send.send(index);
                ctx.request_repaint();
            });
            match job {
                Ok(_) => self.index = IndexState::Building(receive),
                Err(e) => {
                    log::warn!("couldn't start looking for the installed fonts: {e}");
                    self.index = IndexState::Ready(Arc::new(FontIndex::default()));
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = ctx;
    }

    /// Take in what the threads brought, start reading what is wanted, and give egui the faces
    /// it doesn't have yet.
    fn pump(&mut self, ctx: &egui::Context) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let IndexState::Building(receive) = &self.index {
                match receive.try_recv() {
                    Ok(index) => self.index = IndexState::Ready(Arc::new(index)),
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => self.index = IndexState::Ready(Arc::new(FontIndex::default())),
                }
            }
            if let Some(receive) = &self.loading {
                match receive.try_recv() {
                    Ok(read) => {
                        self.loading = None;
                        self.reading.clear();
                        self.take(read);
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.loading = None;
                        self.lost();
                    }
                }
            }
            self.start_reading(ctx);
        }
        // A face that is ready and not among egui's fonts (they were given anew) is given again.
        if !self.dirty && self.ready > 0 && active(ctx) < self.ready {
            self.dirty = true;
        }
        if self.dirty {
            self.install(ctx);
        }
    }

    /// What the reading thread had in hand is not to be had: it went, or never started.
    fn lost(&mut self) {
        for key in std::mem::take(&mut self.reading) {
            self.set(key, FaceState::Missing);
        }
    }

    /// Read the next faces wanted, on a thread of their own (at once, for tests).
    #[cfg(not(target_arch = "wasm32"))]
    fn start_reading(&mut self, ctx: &egui::Context) {
        if self.loading.is_some() || self.queue.is_empty() {
            return;
        }
        let Some(index) = self.index() else { return };
        let take = self.queue.len().min(BATCH);
        let batch: Vec<(String, bool)> = self.queue.drain(..take).collect();
        self.reading = batch.clone();
        let (room, budget) = (MAX_FACES.saturating_sub(self.read), MAX_BYTES.saturating_sub(self.bytes));
        let known: Vec<FaceFile> = self.files.keys().cloned().collect();
        let read = move || {
            let (mut room, mut budget) = (room, budget);
            let mut done: HashMap<FaceFile, Arc<egui::FontData>> = HashMap::new();
            let mut out: Read = Vec::new();
            for key in batch {
                let Some(face) = index.face(&key.0, key.1) else {
                    out.push((key, None));
                    continue;
                };
                let file: FaceFile = (face.path.clone(), face.index);
                if let Some(data) = done.get(&file) {
                    out.push((key, Some((file, data.clone()))));
                    continue;
                }
                // Read under another name since this batch began: the store has it.
                if known.contains(&file) || room == 0 {
                    out.push((key, None));
                    continue;
                }
                let data = crate::theme::read_face(&face.path, MAX_FACE_BYTES.min(budget), face.index).map(|bytes| {
                    room = room.saturating_sub(1);
                    budget = budget.saturating_sub(bytes.len() as u64);
                    let mut data = egui::FontData::from_owned(bytes);
                    data.index = face.index;
                    Arc::new(data)
                });
                if let Some(data) = &data {
                    done.insert(file.clone(), data.clone());
                }
                out.push((key, data.map(|data| (file, data))));
            }
            out
        };
        if self.inline {
            let out = read();
            self.reading.clear();
            self.take(out);
            return;
        }
        let (send, receive) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        match std::thread::Builder::new().name("font-read".into()).spawn(move || {
            let _ = send.send(read());
            ctx.request_repaint();
        }) {
            Ok(_) => self.loading = Some(receive),
            Err(e) => {
                log::warn!("couldn't start reading a font: {e}");
                self.lost();
            }
        }
    }

    /// Faces read: ready, or not to be had.
    #[cfg(not(target_arch = "wasm32"))]
    fn take(&mut self, read: Read) {
        for (key, data) in read {
            match data {
                Some((file, data)) => {
                    if !self.files.contains_key(&file) {
                        self.read = self.read.saturating_add(1);
                        self.bytes = self.bytes.saturating_add((*data.font).as_ref().len() as u64);
                        self.files.insert(file, data.clone());
                    }
                    self.set(key, FaceState::Ready(data));
                }
                None => {
                    // A face this batch skipped because the store already had its file.
                    let had = self.index().and_then(|index| index.face(&key.0, key.1).map(|f| (f.path.clone(), f.index)));
                    match had.and_then(|file| self.files.get(&file)).cloned() {
                        Some(data) => self.set(key, FaceState::Ready(data)),
                        None => self.set(key, FaceState::Missing),
                    }
                }
            }
        }
    }

    /// Give egui every face that is ready and that its fonts don't have yet, each under a family
    /// of its own with the window's fonts behind it for the letters it lacks. egui takes them in
    /// at the start of the next pass, for which a repaint is asked. (The fonts given are egui's
    /// own with these added: nothing else in the program changes its fonts after the first
    /// frame, `theme::install_fonts`.)
    fn install(&mut self, ctx: &egui::Context) {
        let mut defs: FontDefinitions = ctx.fonts(|f| f.definitions().clone());
        let mut added = false;
        for ((name, bold), state) in &self.faces {
            let FaceState::Ready(data) = state else { continue };
            let family = family_of(name, *bold);
            if defs.families.contains_key(&family) {
                continue;
            }
            let face = format!("{PREFIX}{name}{}", if *bold { ":bold" } else { "" });
            let behind = if *bold { FontFamily::Name("bold".into()) } else { FontFamily::Proportional };
            let mut chain = vec![face.clone()];
            chain.extend(defs.families.get(&behind).or_else(|| defs.families.get(&FontFamily::Proportional)).cloned().unwrap_or_default());
            defs.font_data.insert(face, data.clone());
            defs.families.insert(family, chain);
            added = true;
        }
        if added {
            ctx.set_fonts(defs);
            // The pass that takes them in, and lays the messages out again.
            ctx.request_repaint();
        } else {
            // Everything ready is in egui's fonts.
            self.dirty = false;
        }
    }
}

/// How many message faces egui's fonts have.
fn active(ctx: &egui::Context) -> usize {
    ctx.fonts(|f| f.definitions().families.keys().filter(|k| matches!(k, FontFamily::Name(n) if n.starts_with(PREFIX))).count())
}

/// Let go of the faces read for messages (the window is out of sight, [`crate::out_of_sight`]):
/// egui's fonts go back to the window's own, and a face is read again, and counted against the
/// caps again, when a message is next drawn in it. What is installed stays known (or is still
/// being found). Faces being read are not waited for: what they bring is not kept.
pub fn let_go(ctx: &egui::Context) {
    let store = Store::of(ctx);
    let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
    if store.files.is_empty() && store.faces.is_empty() && store.queue.is_empty() {
        return;
    }
    let index = std::mem::replace(&mut store.index, IndexState::NotAsked);
    let inline = store.inline;
    *store = Store { index, inline, ..Store::default() };
    drop(store);
    let mut defs: FontDefinitions = ctx.fonts(|f| f.definitions().clone());
    let before = defs.families.len();
    defs.families.retain(|family, _| !matches!(family, FontFamily::Name(n) if n.starts_with(PREFIX)));
    defs.font_data.retain(|name, _| !name.starts_with(PREFIX));
    if defs.families.len() != before {
        ctx.set_fonts(defs);
    }
}

/// Once a frame where messages are shown: take in what the threads brought and give egui the
/// faces that are ready.
pub fn tick(ctx: &egui::Context) {
    let store = Store::of(ctx);
    let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
    store.pump(ctx);
}

/// What the fonts a layout is made with have come to, as a number that changes when they do: the
/// message faces egui has taken in, the fonts that could not be had, and whether the installed
/// fonts are known. A layout made with other fonts is made again.
pub fn generation(ctx: &egui::Context) -> u64 {
    let active = active(ctx);
    let store = Store::of(ctx);
    let store = store.lock().unwrap_or_else(|e| e.into_inner());
    let known = matches!(store.index, IndexState::Ready(_));
    (active as u64) | ((store.missing as u64 & 0xff_ffff) << 24) | (u64::from(known) << 48)
}

/// Picks the fonts of a layout's spans, from what egui has at the start of the layout.
pub struct Chooser {
    /// The families of message faces and of the composer's fonts (#582) egui has.
    active: HashSet<FontFamily>,
    index: Option<Arc<FontIndex>>,
    /// The installed fonts are not known yet, but will be.
    index_coming: bool,
    /// Faces that are not to be had.
    missing: HashSet<(String, bool)>,
    /// Faces that were wanted and not had, and whether the installed fonts were wanted.
    wants: HashSet<(String, bool)>,
    wants_index: bool,
    /// The picks of lists already seen, for regular and for bold text.
    remembered: [HashMap<String, Pick>; 2],
}

impl Chooser {
    /// The fonts as egui has them now, and the installed fonts as far as they are known.
    pub fn new(ctx: &egui::Context) -> Chooser {
        let active: HashSet<FontFamily> = ctx.fonts(|f| {
            f.definitions()
                .families
                .keys()
                .filter(|k| matches!(k, FontFamily::Name(n) if n.starts_with(PREFIX) || n.starts_with("font-")))
                .cloned()
                .collect()
        });
        let store = Store::of(ctx);
        let store = store.lock().unwrap_or_else(|e| e.into_inner());
        let missing = store.faces.iter().filter(|(_, s)| matches!(s, FaceState::Missing)).map(|(k, _)| k.clone()).collect();
        Chooser { active, index: store.index(), index_coming: store.index_coming(), missing, ..Chooser::none() }
    }

    /// No fonts but the window's: for a layout made without a window.
    pub fn none() -> Chooser {
        Chooser {
            active: HashSet::new(),
            index: None,
            index_coming: false,
            missing: HashSet::new(),
            wants: HashSet::new(),
            wants_index: false,
            remembered: [HashMap::new(), HashMap::new()],
        }
    }

    /// What text in the fonts `list` (a span's, lower case and joined by commas) is drawn in.
    pub fn pick(&mut self, list: &str, bold: bool) -> Pick {
        let [regular, heavy] = &self.remembered;
        if let Some(pick) = if bold { heavy } else { regular }.get(list) {
            return pick.clone();
        }
        let (pick, settled) = self.choose(list, bold);
        let [regular, heavy] = &mut self.remembered;
        let remembered = if bold { heavy } else { regular };
        if settled && remembered.len() < REMEMBERED {
            remembered.insert(list.to_string(), pick.clone());
        }
        pick
    }

    /// The pick, and whether it stays so for this layout.
    fn choose(&mut self, list: &str, bold: bool) -> (Pick, bool) {
        for name in list.split(',').take(mailcraft_html::MAX_FAMILIES) {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let lookup = match generic(name) {
                Some(Generic::Mono) => Lookup::Found(Pick::Mono),
                Some(Generic::Own) => Lookup::Found(Pick::Own),
                Some(Generic::Serif) => {
                    let mut found = Lookup::Found(Pick::Own);
                    for serif in SERIF {
                        match self.installed(serif, bold) {
                            Lookup::Absent => continue,
                            other => {
                                found = other;
                                break;
                            }
                        }
                    }
                    found
                }
                None => self.installed(name, bold),
            };
            match lookup {
                Lookup::Found(pick) => return (pick, true),
                Lookup::Coming => return (Pick::Own, false),
                Lookup::Absent => continue,
            }
        }
        (Pick::Own, true)
    }

    /// Whether `name` is installed, and how its face stands.
    fn installed(&mut self, name: &str, bold: bool) -> Lookup {
        // The composer's fonts, read at start (#582).
        if let Some(font) = Font::parse(name).filter(|f| *f != Font::System) {
            let family = crate::theme::document_family(font, bold);
            if self.active.contains(&family) {
                return Lookup::Found(Pick::Family(family));
            }
            if bold && self.active.contains(&crate::theme::document_family(font, false)) {
                // Installed, without a bold face.
                return Lookup::Found(Pick::Own);
            }
        }
        let Some(index) = &self.index else {
            if self.index_coming {
                self.wants_index = true;
                return Lookup::Coming;
            }
            return Lookup::Absent;
        };
        if !index.has(name) {
            return Lookup::Absent;
        }
        if index.face(name, bold).is_none() {
            // Bold text in a family with no bold face: the window's bold (#582).
            return Lookup::Found(Pick::Own);
        }
        let key = (name.to_string(), bold);
        if self.missing.contains(&key) {
            return Lookup::Absent;
        }
        let family = family_of(name, bold);
        if self.active.contains(&family) {
            return Lookup::Found(Pick::Family(family));
        }
        self.wants.insert(key);
        Lookup::Coming
    }

    /// Hand what was wanted to the window's fonts: the installed fonts looked for, faces read.
    pub fn finish(self, ctx: &egui::Context) {
        if !self.wants_index && self.wants.is_empty() {
            return;
        }
        let store = Store::of(ctx);
        let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
        if self.wants_index {
            store.ask_index(ctx);
        }
        for key in self.wants {
            store.want(key);
        }
        store.pump(ctx);
    }
}

#[cfg(test)]
#[path = "fonts_tests.rs"]
mod fonts_tests;
