// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The fonts of this computer that text is drawn in: the ones a message asks for (#600) and the
//! ones the composer and the signature editor offer (#1493). Its text is drawn in the first font
//! of its `font-family` that is installed on this computer, and nothing is ever fetched for it.
//!
//! - A name is looked up in the fonts installed here ([`FontIndex`], made once, on a thread of its
//!   own, the first time a message names a font or a font menu opens), in the order the message
//!   gives them; the first that is installed wins, and a face that can't be read counts as not
//!   installed.
//! - `serif` is the first of this system's serif fonts that is installed ([`SERIF`]); `monospace`
//!   is the window's own monospaced font; `sans-serif`, `system-ui`, `cursive`, `fantasy` and the
//!   other generic families are the window's own font, and so is a list none of whose fonts is
//!   installed.
//! - Bold text is drawn in the family's bold face; a family without one draws it in the window's
//!   bold, as the composer does (#582): egui can't make a face bold itself — except in a family
//!   whose characters are as wide as each other, which keeps its own regular face so its columns
//!   stay lined up (#969).
//! - A face is read whole only when text is to be drawn in it, on a thread of its own, and
//!   then kept for the life of the program: at most [`MAX_FACES`] faces and [`MAX_BYTES`] bytes,
//!   each at most [`MAX_FACE_BYTES`]; past those, a font counts as not installed.
//! - The faces of the composer's six families, read at start (#582), are used as they are.
//!
//! A face is drawn with only once egui has taken it in, the pass after it was given (naming a
//! family egui doesn't have would stop the program): until then the text is laid out in the
//! window's font, and laid out again when the fonts change ([`generation`]). The browser has no
//! fonts folder: there `monospace` is all that changes anything, and a font menu has nothing to
//! list.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use egui::{FontDefinitions, FontFamily};
use mailcraft_model::rich::Built;

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
    /// How many times the fonts had been looked at when that batch started: a failure from
    /// before the last look is not a word about what is on disk now (#1493).
    reading_rev: u64,
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
    /// How many times the fonts installed here have been looked for, and how many times a face
    /// has been let go of so that it is asked for again: what a layout is made with (#1493).
    revision: u64,
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
        // A look of its own is the fonts changing, whether or not it finds anything new: a layout
        // that gave up on a family asks again (#1493).
        self.revision = self.revision.wrapping_add(1);
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
        {
            // The browser has no fonts folder to look in: the list is known, and empty. A menu
            // that is told the index is still being made would say so for ever (#1493).
            let _ = ctx;
            self.index = IndexState::Ready(Arc::new(FontIndex::default()));
        }
    }

    /// Take in what the threads brought, start reading what is wanted, and give egui the faces
    /// it doesn't have yet.
    fn pump(&mut self, ctx: &egui::Context) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let IndexState::Building(receive) = &self.index {
                match receive.try_recv() {
                    Ok(index) => {
                        self.index = IndexState::Ready(Arc::new(index));
                        self.revision = self.revision.wrapping_add(1);
                    }
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
        let stale = self.reading_rev != self.revision;
        for key in std::mem::take(&mut self.reading) {
            if stale {
                self.revision = self.revision.wrapping_add(1);
                self.faces.remove(&key);
            } else {
                self.set(key, FaceState::Missing);
            }
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
        self.reading_rev = self.revision;
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
        let stale = self.reading_rev != self.revision;
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
                        // The fonts were looked at again while this batch was out: the file may
                        // have been repaired since, so the face is asked for again rather than
                        // kept as not to be had. Letting it go is itself a change in what a layout
                        // is made with, or the layout that settled on the fallback would never be
                        // made again to ask (#1493).
                        None if stale => {
                            self.revision = self.revision.wrapping_add(1);
                            self.faces.remove(&key);
                        }
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
        // While the fonts installed here are being looked at again, nothing is known about which
        // families are fixed width: a face given now would get the window's proportional fonts
        // behind it, and egui would keep that chain, because a family it has is given again. The
        // faces wait for the look to end (#1493).
        if !matches!(self.index, IndexState::Ready(_)) {
            return;
        }
        let mut defs: FontDefinitions = ctx.fonts(|f| f.definitions().clone());
        let index = self.index();
        let mut added = false;
        for ((name, bold), state) in &self.faces {
            let FaceState::Ready(data) = state else { continue };
            let family = family_of(name, *bold);
            if defs.families.contains_key(&family) {
                continue;
            }
            let face = format!("{PREFIX}{name}{}", if *bold { ":bold" } else { "" });
            // What draws the letters this family hasn't got. A family whose characters are as wide
            // as each other keeps the window's monospaced fonts behind it, so that text lined up
            // with spaces stays lined up; the window's own bold stands behind a bold of the rest.
            let fixed = index.as_ref().is_some_and(|index| index.fixed(name));
            let behind = if fixed {
                FontFamily::Monospace
            } else if *bold {
                FontFamily::Name("bold".into())
            } else {
                FontFamily::Proportional
            };
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

/// Once a frame where messages are shown — and once for a `ui.fonts` on the control port, which
/// may be asked in a window that shows nothing: take in what the threads brought and give egui the
/// faces that are ready.
pub fn tick(ctx: &egui::Context) {
    let store = Store::of(ctx);
    let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
    store.pump(ctx);
}

/// What the fonts a layout is made with have come to, as a number that changes when they do: the
/// message faces egui has taken in, the fonts that could not be had, whether the installed fonts
/// are known, and how many times they have been looked for or a face let go of to be asked for
/// again. A layout made with other fonts is made again.
pub fn generation(ctx: &egui::Context) -> u64 {
    let active = active(ctx);
    let store = Store::of(ctx);
    let store = store.lock().unwrap_or_else(|e| e.into_inner());
    let known = matches!(store.index, IndexState::Ready(_));
    (active as u64) | ((store.missing as u64 & 0xff_ffff) << 24) | (u64::from(known) << 48) | ((store.revision & 0x7f_ff) << 49)
}

/// Families that are like one another, each with the families it may stand in for, best first: the
/// metric-compatible clones the free desktops ship in place of the Windows families, and the
/// fixed-width families that stand in for one another. Only used when the family a message asks
/// for isn't installed here (#1493).
const COUSINS: [(&str, &[&str]); 15] = [
    ("courier", &["courier new", "liberation mono", "cousine"]),
    ("geneva", &["verdana"]),
    ("carlito", &["calibri"]),
    ("calibri", &["carlito"]),
    ("helvetica", &["arial", "liberation sans", "arimo"]),
    ("arial", &["liberation sans", "arimo", "helvetica"]),
    ("liberation sans", &["arial", "arimo"]),
    ("arimo", &["arial", "liberation sans"]),
    ("gelasio", &["georgia"]),
    ("georgia", &["gelasio"]),
    ("consolas", &["menlo", "liberation mono", "cousine", "courier new"]),
    ("menlo", &["consolas", "liberation mono", "cousine", "courier new"]),
    ("cousine", &["liberation mono", "courier new"]),
    ("liberation mono", &["courier new", "cousine"]),
    ("dejavu sans mono", &["liberation mono", "courier new"]),
];

/// The families `name` may be drawn in when it isn't installed here, best first.
fn cousins(name: &str) -> &'static [&'static str] {
    COUSINS.iter().find(|(one, _)| *one == name).map_or(&[], |(_, list)| *list)
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
                None => {
                    // A family that isn't here is drawn in the one it is like, if that is: a
                    // message asking for Carlito keeps its look where Calibri is, and one asking
                    // for Consolas keeps its columns where any fixed family is. Only the drawing
                    // looks around: the name that goes out is the name that came in (#1493).
                    let mut lookup = self.installed(name, bold);
                    for cousin in cousins(name) {
                        if !matches!(lookup, Lookup::Absent) {
                            break;
                        }
                        lookup = self.installed(cousin, bold);
                    }
                    lookup
                }
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
        // The composer's six families, read at start (#582).
        if let Some(built) = Built::from_key(name) {
            if built == Built::System {
                // The window's own font: text asked for it is already drawn in it, and reading
                // Segoe UI a second time as a message face would only cost its weight in memory.
                return Lookup::Found(Pick::Own);
            }
            let family = crate::theme::document_family(built, bold);
            if self.active.contains(&family) {
                return Lookup::Found(Pick::Family(family));
            }
            let regular = crate::theme::document_family(built, false);
            if bold && self.active.contains(&regular) {
                // Installed, without a bold face: the window's bold, but a fixed-width font keeps
                // its regular face, so text art keeps its columns (#969).
                return Lookup::Found(if built.is_fixed_width() { Pick::Family(regular) } else { Pick::Own });
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
            // Bold text in a family with no bold face: the window's bold (#582) — except in a
            // family whose characters are as wide as each other, which keeps its own regular face
            // so that its columns stay lined up (#969).
            if bold && index.fixed(name) {
                let regular = family_of(name, false);
                if self.active.contains(&regular) {
                    return Lookup::Found(Pick::Family(regular));
                }
                let regular_key = (name.to_string(), false);
                if self.missing.contains(&regular_key) {
                    // Its regular face was read and couldn't be: the list goes on to what comes
                    // after this family, which may be `monospace` (#1493).
                    return Lookup::Absent;
                }
                self.wants.insert(regular_key);
                return Lookup::Coming;
            }
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

/// Start looking for the fonts installed here, if that hasn't begun: a font menu asks when it
/// opens, so that the list is there a moment later. What is already known is kept.
pub fn ask(ctx: &egui::Context) {
    let store = Store::of(ctx);
    let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
    store.ask_index(ctx);
}

/// Whether the fonts installed here may yet be known: on a system with fonts folders, until the
/// index is made ([`Store::index_coming`]).
pub fn coming(ctx: &egui::Context) -> bool {
    let store = Store::of(ctx);
    let store = store.lock().unwrap_or_else(|e| e.into_inner());
    store.index_coming()
}

/// The installed fonts as the index has them, when they are known: the families a menu lists, in
/// order and already folded for searching, kept whole so that a menu open for a minute doesn't
/// build three thousand names again every frame (#1493).
pub fn index(ctx: &egui::Context) -> Option<Arc<FontIndex>> {
    let store = Store::of(ctx);
    let store = store.lock().unwrap_or_else(|e| e.into_inner());
    store.index()
}

/// The families installed here, each by the name it gives itself, in order; `None` while they are
/// still being found. What a font menu lists.
pub fn names(ctx: &egui::Context) -> Option<Vec<String>> {
    let store = Store::of(ctx);
    let store = store.lock().unwrap_or_else(|e| e.into_inner());
    let index = store.index()?;
    Some(index.names().iter().map(|n| (*n).to_string()).collect())
}

/// What kind of family `name` is, as its own file says: the generic family written after it in the
/// `font-family` that goes out, so that a recipient without it gets a font of the same kind —
/// monospace when its characters are as wide as each other, serif when its class is one of the
/// serif ones, `cursive` for a script or a blackletter that is neither. A family whose file says
/// nothing is taken for a sans-serif one, which is what most are.
pub fn generic_family(ctx: &egui::Context, name: &str) -> &'static str {
    let key = name.to_lowercase();
    let store = Store::of(ctx);
    let store = store.lock().unwrap_or_else(|e| e.into_inner());
    match store.index() {
        Some(index) if index.fixed(&key) => "monospace",
        Some(index) if index.serif(&key) => "serif",
        Some(index) if index.script(&key) => "cursive",
        _ => "sans-serif",
    }
}

/// Look for the fonts installed here again, on a thread of their own: a font installed since the
/// window started comes into the menu. Faces already read are kept — a family taken away stays
/// drawable until the window lets go of what it keeps to show things again quickly
/// ([`let_go`]) — and a family that is new, or has new faces, is read when next it is drawn.
pub fn rescan(ctx: &egui::Context) {
    let store = Store::of(ctx);
    let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
    // A face that couldn't be read is forgotten: the file may have been replaced, and a family
    // kept as unreadable would otherwise stay unreadable for the life of the window (#1493).
    let before = store.faces.len();
    store.faces.retain(|_, state| !matches!(state, FaceState::Missing));
    store.missing = store.missing.saturating_sub(before - store.faces.len());
    store.index = IndexState::NotAsked;
    store.ask_index(ctx);
}

/// What is installed here, for a script or an agent (`ui.fonts`, `docs/control-protocol.md`):
/// whether it is still being found, and the families by the names they give themselves, at most
/// [`INSPECT_NAMES`] of them.
pub fn inspect(ctx: &egui::Context) -> serde_json::Value {
    const INSPECT_NAMES: usize = 2_000;
    let coming = coming(ctx);
    let families: Vec<String> = names(ctx).map(|n| n.into_iter().take(INSPECT_NAMES).collect()).unwrap_or_default();
    serde_json::json!({"coming": coming, "families": families})
}

#[cfg(test)]
#[path = "fonts_tests.rs"]
mod fonts_tests;
