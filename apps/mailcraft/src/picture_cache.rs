// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pictures from the web, kept on disk between runs.
//!
//! A picture on the web tells whoever serves it that it was asked for. One the owner asked for
//! once (for a message, a sender, a domain, or all mail) is therefore kept here, so that reading
//! the message again after the program was closed asks nobody: it costs no waiting, and tells
//! the sender nothing new. Servers are not asked how long they would like a picture kept (a
//! picture that counts openings says "never"); every picture is kept alike, for [`MAX_AGE`].
//!
//! ```text
//! <settings>/pictures.cache/      beside the mailbox's own content folder
//!     index                       a line per address a picture came from:
//!                                 <SHA-256 of the address> <SHA-256 of the bytes> <length> <fetched> <last used>
//!     3f/3f2a…                    the bytes as they came, in a file named by their SHA-256
//!                                 (64 hex digits, no extension), in a folder named by the first two
//! ```
//!
//! - **What is written:** the bytes of a picture, and the two hashes, the length and the times.
//!   Not the address (only its hash: the folder doesn't list what was read), not the message,
//!   not who sent it, nothing a server said besides the bytes. The same picture at two addresses
//!   is one file.
//! - **Only pictures:** bytes are kept when they begin as a PNG, a JPEG, a GIF or a WebP does
//!   (or an ICO, a web site's icon, for a sender's picture: [`Cache::get_as`]) and are no
//!   larger than a download may be; the window, which decodes them, takes back what turns
//!   out to be none ([`Cache::forget`]). Nothing here is ever run, opened by another program on
//!   our say-so, or served to anyone: a file is read back as bytes, and only when it is a plain
//!   file of the length and SHA-256 its line names. What comes back goes to the window the way a
//!   download does, and is looked at there exactly as warily.
//! - **Bounded:** at most [`MAX_PICTURES`] pictures and [`MAX_TOTAL`] bytes, the ones used
//!   longest ago going first, and none older than [`MAX_AGE`]. File › Options says how much is
//!   kept and clears it ([`Cache::clear`]).
//! - **Hostile:** the folder is as hostile as anything else on disk. A line of the index that
//!   is not five fields of the right shape is skipped; a name is 64 hex digits or nothing, so
//!   nothing read from the index can become another path; sizes and counts are capped; a file
//!   that is missing, a link, too long or not what its name says is forgotten, not shown.
//! - **Private:** on Unix the folder and every file in it are its owner's alone (0700 and
//!   0600), as the mailbox is; a folder an earlier version left open to others is closed.
//!
//! There is none for the sample mailbox or a run without preferences, which keep nothing.

use std::collections::HashMap;
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mailcraft_engine::store::{Digest, create_private_dir_all, create_private_replacing, make_private};

/// The folder's name, in the settings folder.
pub const FOLDER: &str = "pictures.cache";
/// Most pictures kept.
pub const MAX_PICTURES: usize = 4_000;
/// Most bytes kept, all pictures together.
pub const MAX_TOTAL: u64 = 256 * 1024 * 1024;
/// Largest picture kept (what a download may be at most).
pub const MAX_PICTURE: usize = mailcraft_net::pictures::MAX_BYTES;
/// How long a picture is kept from when it was downloaded, in seconds: thirty days.
pub const MAX_AGE: u64 = 30 * 86_400;
/// Largest index read.
const MAX_INDEX: u64 = 2 * 1024 * 1024;
/// Longest address a picture is kept for (longer ones are not downloaded either).
const MAX_ADDRESS: usize = 16 * 1024;
/// The index is written at most this often while pictures come in; the rest waits for
/// [`Cache::flush`].
const WRITE_EVERY: Duration = Duration::from_secs(3);
/// A file nothing names is left alone for this long after it was written: a download in flight
/// writes its bytes before the index names them.
const GRACE: Duration = Duration::from_secs(10 * 60);
/// Directory entries one tidying looks at.
const MAX_ENTRIES: usize = 100_000;

/// What the index says of one address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    content: Digest,
    len: u64,
    /// When it was downloaded, and when it was last asked for (Unix seconds).
    fetched: u64,
    used: u64,
}

#[derive(Default)]
struct State {
    /// The index was read (it is read when the first picture is asked for).
    loaded: bool,
    /// By the SHA-256 of the address.
    entries: HashMap<Digest, Entry>,
    /// Something changed since the index was last written.
    dirty: bool,
    written: Option<Instant>,
}

/// The pictures kept in one folder, shared between the threads that download.
pub struct Cache {
    dir: PathBuf,
    state: Mutex<State>,
    /// One at a time writes the index.
    writing: Mutex<()>,
    /// How many addresses have a picture kept, and how many bytes the files take: what
    /// [`Cache::usage`] says without waiting for anyone.
    count: AtomicUsize,
    bytes: AtomicU64,
    /// For the names of files being written.
    serial: AtomicU64,
    /// A thread is waiting to write the index a little later.
    later: AtomicBool,
    /// The bounds ([`MAX_PICTURES`] and [`MAX_TOTAL`]; smaller in the tests).
    max_pictures: usize,
    max_total: u64,
    /// How far ahead of the system's clock this cache's is, in seconds: none, but in the tests,
    /// which can't wait a month.
    ahead: AtomicU64,
    /// MailCraft's data is being removed from this computer (#177): nothing is written any more,
    /// whatever download is still in flight.
    stopped: AtomicBool,
}

/// The time, in Unix seconds.
fn clock() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

/// Whether bytes begin the way a picture that is shown begins: PNG, JPEG, GIF or WebP.
pub fn looks_like_a_picture(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(b"\xff\xd8\xff")
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(&b"WEBP"[..]))
}

/// What an address is kept under: the SHA-256 of it as the message wrote it, less the white
/// space round it. `None` for one that is no address to keep anything for.
fn key(url: &str) -> Option<Digest> {
    let url = url.trim();
    (!url.is_empty() && url.len() <= MAX_ADDRESS).then(|| Digest::of(url.as_bytes()))
}

/// One line of the index; `None` for anything that is not one.
fn parse_line(line: &str) -> Option<(Digest, Entry)> {
    let mut fields = line.split(' ');
    let address = Digest::parse(fields.next()?)?;
    let content = Digest::parse(fields.next()?)?;
    let number = |field: Option<&str>| {
        field.filter(|f| !f.is_empty() && f.len() <= 20 && f.bytes().all(|b| b.is_ascii_digit())).and_then(|f| f.parse::<u64>().ok())
    };
    let (len, fetched, used) = (number(fields.next())?, number(fields.next())?, number(fields.next())?);
    let fits = fields.next().is_none() && len > 0 && len <= MAX_PICTURE as u64;
    fits.then_some((address, Entry { content, len, fetched, used }))
}

/// The index as text, a line an address.
fn write_lines(entries: &HashMap<Digest, Entry>) -> String {
    let mut lines: Vec<String> =
        entries.iter().map(|(address, e)| format!("{} {} {} {} {}\n", address.hex(), e.content.hex(), e.len, e.fetched, e.used)).collect();
    // In an order that doesn't change from one writing to the next.
    lines.sort_unstable();
    lines.concat()
}

/// What an index read from disk comes to at `now`: its lines that are lines, none too old or
/// from a time that hasn't come, and no more of them than `most`.
fn read_lines(text: &str, now: u64, most: usize) -> HashMap<Digest, Entry> {
    let mut entries = HashMap::new();
    for line in text.lines().take(MAX_PICTURES.saturating_mul(4)) {
        let Some((address, entry)) = parse_line(line) else { continue };
        let timely = entry.fetched <= now.saturating_add(86_400) && now.saturating_sub(entry.fetched) <= MAX_AGE;
        if timely && entries.len() < most.min(MAX_PICTURES) {
            entries.insert(address, entry);
        }
    }
    entries
}

/// The addresses to forget so that what is left is within the bounds: the ones used longest ago.
fn over_the_bounds(entries: &HashMap<Digest, Entry>, max_pictures: usize, max_total: u64) -> Vec<Digest> {
    let total = |entries: &HashMap<Digest, Entry>, gone: &[Digest]| -> u64 {
        // A file two addresses share is counted once.
        let mut files: HashMap<Digest, u64> = HashMap::new();
        for (address, entry) in entries {
            if !gone.contains(address) {
                files.insert(entry.content, entry.len);
            }
        }
        files.values().sum()
    };
    if entries.len() <= max_pictures && total(entries, &[]) <= max_total {
        return Vec::new();
    }
    let mut oldest: Vec<(u64, u64, Digest)> = entries.iter().map(|(address, e)| (e.used, e.fetched, *address)).collect();
    oldest.sort_unstable();
    let mut gone = Vec::new();
    let mut left = total(entries, &[]);
    for (_, _, address) in oldest {
        if entries.len().saturating_sub(gone.len()) <= max_pictures && left <= max_total {
            break;
        }
        gone.push(address);
        // (Counted again only when a file may have lost its last address.)
        left = total(entries, &gone);
    }
    gone
}

impl Cache {
    /// The pictures kept in `dir`. Nothing is read or made until a picture is asked for or kept.
    pub fn at(dir: PathBuf) -> Arc<Cache> {
        Cache::bounded(dir, MAX_PICTURES, MAX_TOTAL)
    }

    /// The same, with bounds of its own.
    fn bounded(dir: PathBuf, max_pictures: usize, max_total: u64) -> Arc<Cache> {
        Arc::new(Cache {
            dir,
            state: Mutex::default(),
            writing: Mutex::default(),
            count: AtomicUsize::new(0),
            bytes: AtomicU64::new(0),
            serial: AtomicU64::new(0),
            later: AtomicBool::new(false),
            max_pictures: max_pictures.min(MAX_PICTURES),
            max_total: max_total.min(MAX_TOTAL),
            ahead: AtomicU64::new(0),
            stopped: AtomicBool::new(false),
        })
    }

    /// Write nothing from now on: a picture still being downloaded is not kept, and the index is
    /// not written, so that the folder isn't made again after it was removed (#177).
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// The time as this cache has it, in Unix seconds.
    fn now(&self) -> u64 {
        clock().saturating_add(self.ahead.load(Ordering::Relaxed))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A thread that died holding it left the map as it was: still a map.
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.loaded {
            state.loaded = true;
            // A folder an earlier version made open to others is closed (it is ours): best
            // effort, since a picture not kept private is no reason to keep none.
            if let Err(e) = make_private(&self.dir) {
                log::debug!("the folder of downloaded pictures was not made private: {e}");
            }
            state.entries = self.read_index();
            self.count_up(&state);
        }
        state
    }

    fn read_index(&self) -> HashMap<Digest, Entry> {
        let Some(file) = open_plain(&self.dir.join("index"), MAX_INDEX) else { return HashMap::new() };
        let mut text = String::new();
        // Not UTF-8, or longer than an index gets: no index.
        match file.take(MAX_INDEX.saturating_add(1)).read_to_string(&mut text) {
            Ok(n) if n as u64 <= MAX_INDEX => read_lines(&text, self.now(), self.max_pictures),
            _ => HashMap::new(),
        }
    }

    /// Say again how much is kept.
    fn count_up(&self, state: &State) {
        let mut files: HashMap<Digest, u64> = HashMap::new();
        for entry in state.entries.values() {
            files.insert(entry.content, entry.len);
        }
        self.count.store(state.entries.len(), Ordering::Relaxed);
        self.bytes.store(files.values().sum(), Ordering::Relaxed);
    }

    /// Where the bytes named `content` are kept, and the folder they are in.
    fn path(&self, content: &Digest) -> Option<(PathBuf, PathBuf)> {
        let name = content.hex();
        let folder = self.dir.join(name.get(..2)?);
        let file = folder.join(&name);
        Some((folder, file))
    }

    /// How many pictures are kept, and how many bytes they take. Waits for nobody.
    pub fn usage(&self) -> (usize, u64) {
        // (Read once, so that a window that only ever asks this still learns what is there.)
        if let Ok(state) = self.state.try_lock()
            && !state.loaded
        {
            drop(state);
            drop(self.lock());
        }
        (self.count.load(Ordering::Relaxed), self.bytes.load(Ordering::Relaxed))
    }

    /// The picture kept for `url`, when there is one and it is still what it was.
    pub fn get(&self, url: &str) -> Option<Vec<u8>> {
        self.get_as(url, looks_like_a_picture)
    }

    /// [`Cache::get`], with `looks` saying what begins as a picture: a sender's may be a web
    /// site's icon as well (`sender_images`).
    pub fn get_as(&self, url: &str, looks: fn(&[u8]) -> bool) -> Option<Vec<u8>> {
        let address = key(url)?;
        let entry = {
            let mut state = self.lock();
            let entry = *state.entries.get(&address)?;
            if self.now().saturating_sub(entry.fetched) > MAX_AGE {
                // Its file goes at the next tidying, when no other address names it.
                state.entries.remove(&address);
                state.dirty = true;
                self.count_up(&state);
                return None;
            }
            entry
        };
        // The file is read without holding anything: other downloads go on meanwhile.
        match self.read(&entry, looks) {
            Some(bytes) => {
                let mut state = self.lock();
                if let Some(kept) = state.entries.get_mut(&address) {
                    kept.used = self.now();
                    state.dirty = true;
                }
                Some(bytes)
            }
            None => {
                self.forget_address(&address);
                None
            }
        }
    }

    /// The bytes an entry names, when its file is there and is that: a plain file of that
    /// length with that SHA-256, which begins as a picture does (`looks`).
    fn read(&self, entry: &Entry, looks: fn(&[u8]) -> bool) -> Option<Vec<u8>> {
        let (_, path) = self.path(&entry.content)?;
        let len = usize::try_from(entry.len).ok().filter(|len| *len <= MAX_PICTURE)?;
        let file = open_plain_waiting(&path, entry.len)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(len.checked_add(1)?).ok()?;
        // Never more than was promised, however the file grows while it is read.
        file.take(entry.len.checked_add(1)?).read_to_end(&mut bytes).ok()?;
        (bytes.len() == len && Digest::of(&bytes) == entry.content && looks(&bytes)).then_some(bytes)
    }

    /// Keep `bytes` as the picture at `url`, when they begin as a picture does and are not too
    /// large. Whether they were kept; trouble with the disk only means they were not.
    pub fn put(self: &Arc<Self>, url: &str, bytes: &[u8]) -> bool {
        self.put_as(url, bytes, looks_like_a_picture)
    }

    /// [`Cache::put`], with `looks` saying what begins as a picture ([`Cache::get_as`]).
    pub fn put_as(self: &Arc<Self>, url: &str, bytes: &[u8], looks: fn(&[u8]) -> bool) -> bool {
        let Some(address) = key(url) else { return false };
        if self.is_stopped() || bytes.is_empty() || bytes.len() > MAX_PICTURE || bytes.len() as u64 > self.max_total || !looks(bytes) {
            return false;
        }
        let content = Digest::of(bytes);
        if self.is_stopped() {
            return false;
        }
        if let Err(e) = self.write(&content, bytes) {
            log::debug!("a downloaded picture was not kept: {e}");
            return false;
        }
        let at = self.now();
        let gone = {
            let mut state = self.lock();
            state.entries.insert(address, Entry { content, len: bytes.len() as u64, fetched: at, used: at });
            let gone = over_the_bounds(&state.entries, self.max_pictures, self.max_total);
            let lost: Vec<Entry> = gone.iter().filter_map(|address| state.entries.remove(address)).collect();
            state.dirty = true;
            self.count_up(&state);
            self.unused(&state, &lost)
        };
        self.remove(&gone);
        // The index is written now, or a little later when it was written a moment ago (a
        // message's pictures come in a burst): by a thread that waits, once for all of them.
        if !self.write_index(false) && !self.later.swap(true, Ordering::Relaxed) {
            let cache = self.clone();
            let wait = move || {
                std::thread::sleep(WRITE_EVERY);
                cache.later.store(false, Ordering::Relaxed);
                cache.write_index(true);
            };
            if std::thread::Builder::new().name("picture index".into()).spawn(wait).is_err() {
                self.later.store(false, Ordering::Relaxed);
            }
        }
        true
    }

    /// Write the bytes under their name, when they are not there yet: to a file beside it that
    /// is renamed over, so that a name never holds half a picture.
    fn write(&self, content: &Digest, bytes: &[u8]) -> std::io::Result<()> {
        let (folder, file) = self.path(content).ok_or_else(|| std::io::Error::other("no name"))?;
        let there = |file: &Path| fs::symlink_metadata(file).is_ok_and(|m| m.file_type().is_file() && m.len() == bytes.len() as u64);
        if there(&file) {
            // Its time is renewed, so that a tidying doesn't take it for forgotten.
            let _ = fs::OpenOptions::new().write(true).open(&file).and_then(|f| f.set_modified(SystemTime::now()));
            return Ok(());
        }
        // For the owner alone, as the mailbox is: what was read is nobody else's business.
        create_private_dir_all(&folder)?;
        let serial = self.serial.fetch_add(1, Ordering::Relaxed);
        let partial = folder.join(format!("{}.{}-{serial}.part", content.hex(), std::process::id()));
        let written = create_private_replacing(&partial).and_then(|mut f| f.write_all(bytes).and_then(|()| f.flush())).and_then(|()| {
            // Another thread may have kept it while this one wrote: renaming over a file that is
            // being read is refused on Windows, or leaves it missing for a moment, so don't.
            if there(&file) { Err(std::io::Error::other("kept meanwhile")) } else { fs::rename(&partial, &file) }
        });
        match written {
            Ok(()) => Ok(()),
            // Another thread kept the same picture at the same moment.
            Err(_) if there(&file) => {
                let _ = fs::remove_file(&partial);
                Ok(())
            }
            Err(e) => {
                let _ = fs::remove_file(&partial);
                Err(e)
            }
        }
    }

    /// The files of `lost` entries that no entry names any more.
    fn unused(&self, state: &State, lost: &[Entry]) -> Vec<Digest> {
        lost.iter().map(|entry| entry.content).filter(|content| !state.entries.values().any(|kept| kept.content == *content)).collect()
    }

    fn remove(&self, files: &[Digest]) {
        for content in files {
            if let Some((_, path)) = self.path(content) {
                let _ = fs::remove_file(path);
            }
        }
    }

    fn forget_address(&self, address: &Digest) {
        let gone = {
            let mut state = self.lock();
            let Some(lost) = state.entries.remove(address) else { return };
            state.dirty = true;
            self.count_up(&state);
            self.unused(&state, &[lost])
        };
        self.remove(&gone);
    }

    /// Forget what is kept for `url`.
    pub fn forget(&self, url: &str) {
        if let Some(address) = key(url) {
            self.forget_address(&address);
        }
    }

    /// Forget everything: no picture is kept after this, and the folder holds nothing of ours.
    pub fn clear(&self) -> Result<(), String> {
        {
            let mut state = self.lock();
            state.entries.clear();
            state.dirty = false;
            self.count_up(&state);
        }
        let _writing = self.writing.lock().unwrap_or_else(|e| e.into_inner());
        self.sweep(|_| true);
        for name in ["index", "index.part"] {
            match fs::remove_file(self.dir.join(name)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("The downloaded pictures couldn't all be removed: {e}")),
                _ => {}
            }
        }
        // (The folder itself too, when nothing else is in it.)
        let _ = fs::remove_dir(&self.dir);
        Ok(())
    }

    /// Remove the files in the folder that are ours to remove and that `gone` picks: files named
    /// as a picture's bytes are named (or as one being written is), in folders named by two hex
    /// digits. Anything else that somebody put there is left alone.
    fn sweep(&self, gone: impl Fn(&Path) -> bool) {
        let is_hex = |text: &str, len: usize| text.len() == len && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        let Ok(folders) = fs::read_dir(&self.dir) else { return };
        let mut seen = 0usize;
        // (So many entries and no more, readable or not: a folder that keeps failing to be
        // read is not read for ever.)
        for folder in folders.take(1_024) {
            let Ok(folder) = folder else { continue };
            let name = folder.file_name();
            let real = folder.file_type().is_ok_and(|kind| kind.is_dir());
            if !real || !name.to_str().is_some_and(|name| is_hex(name, 2)) {
                continue;
            }
            let Ok(files) = fs::read_dir(folder.path()) else { continue };
            for file in files {
                seen += 1;
                if seen > MAX_ENTRIES {
                    return;
                }
                let Ok(file) = file else { continue };
                let name = file.file_name();
                let Some(name) = name.to_str() else { continue };
                let ours = is_hex(name, 64) || (name.ends_with(".part") && name.get(..64).is_some_and(|hash| is_hex(hash, 64)));
                if ours && file.file_type().is_ok_and(|kind| kind.is_file()) && gone(&file.path()) {
                    let _ = fs::remove_file(file.path());
                }
            }
            // (It goes when it is empty, and stays when it is not.)
            let _ = fs::remove_dir(folder.path());
        }
    }

    /// Remove the files nothing names any more, when they have been there a while: what a run
    /// that ended between writing a picture and writing the index left behind. Without it the
    /// folder could grow past its bounds with nobody counting.
    pub fn tidy(&self) {
        if self.is_stopped() {
            return;
        }
        let named: std::collections::HashSet<String> = self.lock().entries.values().map(|entry| entry.content.hex()).collect();
        let old = |path: &Path| fs::symlink_metadata(path).and_then(|m| m.modified()).is_ok_and(|at| at.elapsed().is_ok_and(|age| age > GRACE));
        self.sweep(|path| old(path) && !path.file_name().and_then(|name| name.to_str()).is_some_and(|name| named.contains(name)));
    }

    /// Write the index when something changed: at once when `now`, else no more often than
    /// every few seconds. Whether nothing is left to write.
    ///
    /// The folder is not made for it: keeping a picture makes it, and an index of a folder
    /// that is gone (cleared, or taken away under us) names nothing worth writing down.
    fn write_index(&self, now: bool) -> bool {
        if self.is_stopped() {
            return true;
        }
        let _writing = self.writing.lock().unwrap_or_else(|e| e.into_inner());
        let text = {
            let mut state = self.lock();
            if !state.dirty {
                return true;
            }
            if !now && state.written.is_some_and(|at| at.elapsed() < WRITE_EVERY) {
                return false;
            }
            state.dirty = false;
            state.written = Some(Instant::now());
            write_lines(&state.entries)
        };
        let (partial, index) = (self.dir.join("index.part"), self.dir.join("index"));
        let written = create_private_replacing(&partial)
            .and_then(|mut f| f.write_all(text.as_bytes()).and_then(|()| f.flush()))
            .and_then(|()| fs::rename(&partial, &index));
        if let Err(e) = written {
            log::debug!("the index of downloaded pictures was not written: {e}");
            let _ = fs::remove_file(&partial);
            // It is still to be written, the next time something is kept or the window closes.
            self.lock().dirty = true;
        }
        true
    }

    /// Write what is still to be written: when the window closes.
    pub fn flush(&self) {
        // Nothing was ever asked of it: nothing to write.
        if !self.state.lock().unwrap_or_else(|e| e.into_inner()).loaded {
            return;
        }
        self.write_index(true);
    }
}

/// How often, and how long between tries, a file that is not there or will not open is asked
/// for again by [`open_plain_waiting`].
const OPEN_TRIES: u32 = 10;
const OPEN_WAIT: Duration = Duration::from_millis(10);

/// [`open_plain`], tried again for a moment when it fails: a file another thread is just
/// renaming over the same name is, on Windows, refused to a reader or missing for a moment,
/// and a picture that was kept would be taken for one that was not. A file that really is not
/// right costs `OPEN_TRIES` waits once and is forgotten as before.
fn open_plain_waiting(path: &Path, max: u64) -> Option<fs::File> {
    for _ in 1..OPEN_TRIES {
        if let Some(file) = open_plain(path, max) {
            return Some(file);
        }
        std::thread::sleep(OPEN_WAIT);
    }
    open_plain(path, max)
}

/// Open `path` for reading when it is a plain file (not a link) of no more than `max` bytes.
fn open_plain(path: &Path, max: u64) -> Option<fs::File> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() || meta.len() > max {
        return None;
    }
    let file = fs::File::open(path).ok()?;
    // What was opened is what was looked at: a plain file still.
    file.metadata().ok().filter(|meta| meta.is_file() && meta.len() <= max)?;
    Some(file)
}

/// The cache as the window knows it ([`mailcraft_ui_egui::PictureCache`]).
pub struct Kept(pub Arc<Cache>);

impl mailcraft_ui_egui::PictureCache for Kept {
    fn usage(&self) -> (usize, u64) {
        self.0.usage()
    }

    fn forget(&self, url: &str) {
        self.0.forget(url);
    }

    fn clear(&self) -> Result<(), String> {
        self.0.clear()
    }
}

/// Download the picture at `url`, or take it from `cache` when it is kept there; what was
/// downloaded is kept for the next time. On a background thread: it waits for the network.
pub fn picture(url: &str, cache: Option<&Arc<Cache>>) -> Result<Vec<u8>, String> {
    if let Some(bytes) = cache.and_then(|cache| cache.get(url)) {
        return Ok(bytes);
    }
    let bytes = mailcraft_net::pictures::fetch(url).map_err(|e| e.to_string())?;
    if let Some(cache) = cache {
        cache.put(url, &bytes);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of this test's own, removed when the test is done.
    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Temp {
            let dir = std::env::temp_dir().join(format!("mailcraft-pictures-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Temp(dir)
        }

        fn cache(&self) -> Arc<Cache> {
            Cache::at(self.0.join(FOLDER))
        }

        /// A cache on the same folder that keeps less: so many pictures, so many bytes.
        fn small(&self, max_pictures: usize, max_total: u64) -> Arc<Cache> {
            Cache::bounded(self.0.join(FOLDER), max_pictures, max_total)
        }

        /// Every file under the cache's folder, by its path from there.
        fn files(&self) -> Vec<String> {
            let mut out = Vec::new();
            let mut folders = vec![self.0.join(FOLDER)];
            while let Some(folder) = folders.pop() {
                for entry in fs::read_dir(&folder).into_iter().flatten().flatten() {
                    if entry.path().is_dir() {
                        folders.push(entry.path());
                    } else {
                        out.push(entry.path().strip_prefix(self.0.join(FOLDER)).unwrap().to_string_lossy().replace('\\', "/"));
                    }
                }
            }
            out.sort();
            out
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Bytes that begin as a PNG does, `len` of them, different for each `seed`.
    fn png(seed: u8, len: usize) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend((0..len.saturating_sub(8)).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)));
        bytes
    }

    #[test]
    fn a_picture_is_kept_and_comes_back_at_the_next_start() {
        let temp = Temp::new("kept");
        let cache = temp.cache();
        let (url, picture) = ("https://img.example/hero.png?w=600&sig=abc", png(1, 5_000));
        // Nothing is made before anything is kept: asking is no reason for a folder.
        assert_eq!((cache.get(url), cache.usage()), (None, (0, 0)));
        cache.flush();
        assert!(!temp.0.exists());
        assert!(cache.put(url, &picture));
        assert_eq!(cache.get(url).as_deref(), Some(&picture[..]));
        assert_eq!(cache.get(&format!("  {url} \r\n")).as_deref(), Some(&picture[..]));
        assert_eq!(cache.usage(), (1, 5_000));
        // Any other address has nothing, however like it.
        for other in ["https://img.example/hero.png?w=600&sig=abd", "https://img.example/hero.png", "HTTPS://img.example/hero.png?w=600&sig=abc", ""]
        {
            assert_eq!(cache.get(other), None, "{other}");
        }
        // On disk: the index, and the bytes under the name of their own SHA-256, as they came.
        let name = Digest::of(&picture).hex();
        let mut wanted = ["index".to_string(), format!("{}/{name}", &name[..2])];
        wanted.sort();
        assert_eq!(temp.files(), wanted);
        assert_eq!(fs::read(temp.0.join(FOLDER).join(&name[..2]).join(&name)).unwrap(), picture);
        // The index names no address, only hashes, a length and two times.
        let index = fs::read_to_string(temp.0.join(FOLDER).join("index")).unwrap();
        assert!(!index.contains("img.example") && !index.contains("hero") && !index.contains("sig"), "{index}");
        let fields: Vec<&str> = index.trim_end().split(' ').collect();
        assert_eq!((fields.len(), fields[0], fields[1], fields[2]), (5, Digest::of(url.as_bytes()).hex().as_str(), name.as_str(), "5000"));
        assert!(index.ends_with('\n') && index.lines().count() == 1);
        // The next start: a cache on the same folder knows it, and how much there is.
        cache.flush();
        let again = temp.cache();
        assert_eq!(again.usage(), (1, 5_000));
        assert_eq!(again.get(url).as_deref(), Some(&picture[..]));
        // The same picture at another address is the same file, counted once.
        assert!(again.put("https://cdn.example/same.png", &picture));
        assert_eq!((again.usage(), temp.files().len()), ((2, 5_000), 2));
    }

    /// What was read is nobody else's business: the folder and its files are the owner's alone,
    /// and a folder an earlier version left open to others is closed when it is next used.
    #[cfg(unix)]
    #[test]
    fn the_folder_and_its_files_are_the_owners_alone() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let temp = Temp::new("private");
        let picture = png(3, 2_000);
        assert!(temp.cache().put("https://img.example/private.png", &picture));
        let dir = temp.0.join(FOLDER);
        let name = Digest::of(&picture).hex();
        assert_eq!((mode(&temp.0), mode(&dir), mode(&dir.join(&name[..2]))), (0o700, 0o700, 0o700));
        assert_eq!((mode(&dir.join("index")), mode(&dir.join(&name[..2]).join(&name))), (0o600, 0o600));

        // As an earlier version left it: open to be read by others.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let again = temp.cache();
        assert_eq!(again.usage(), (1, 2_000));
        assert_eq!(mode(&dir), 0o700);
    }

    #[test]
    fn only_what_begins_as_a_picture_is_kept() {
        let temp = Temp::new("only");
        let cache = temp.cache();
        let webp = {
            let mut bytes = b"RIFF\x40\0\0\0WEBPVP8L".to_vec();
            bytes.extend([7u8; 60]);
            bytes
        };
        for (name, bytes) in
            [("png", png(2, 64)), ("jpeg", b"\xff\xd8\xff\xe0\0\x10JFIF\0rest".to_vec()), ("gif", b"GIF89a\x01\0\x01\0rest".to_vec()), ("webp", webp)]
        {
            assert!(looks_like_a_picture(&bytes) && cache.put(&format!("https://img.example/{name}"), &bytes), "{name}");
        }
        let too_large = png(3, MAX_PICTURE + 1);
        for (what, bytes) in [
            ("nothing", &b""[..]),
            ("a web page", b"<!DOCTYPE html><html><script>alert(1)</script></html>"),
            ("a drawing with a script", b"<svg xmlns='http://www.w3.org/2000/svg'><script>alert(1)</script></svg>"),
            ("a program", b"MZ\x90\0\x03\0\0\0\x04\0\0\0\xff\xff\0\0"),
            ("a script", b"#!/bin/sh\nrm -rf ~\n"),
            ("a shortcut", b"[InternetShortcut]\r\nURL=file:///C:/Windows/System32/calc.exe\r\n"),
            ("an archive", b"PK\x03\x04\x14\0\0\0\x08\0"),
            ("riff that is no webp", b"RIFF\x24\0\0\0WAVEfmt "),
            ("almost a png", b"\x89PNG\r\n\x1a"),
            ("a web site's icon, which no message shows", b"\0\0\x01\0\x01\0rest"),
            ("too large", &too_large[..]),
        ] {
            assert!(!cache.put("https://img.example/other", bytes), "{what}");
        }
        assert_eq!(cache.get("https://img.example/other"), None);
        assert!(!cache.put("", &png(4, 64)) && !cache.put(&"a".repeat(MAX_ADDRESS + 1), &png(4, 64)));
        assert_eq!(cache.usage().0, 4);
        // Every file kept has a name of 64 hex digits and nothing after it: nothing to run.
        for file in temp.files().iter().filter(|file| *file != "index") {
            let name = file.rsplit('/').next().unwrap();
            assert!(name.len() == 64 && !name.contains('.') && file.starts_with(&name[..2]), "{file}");
        }
    }

    #[test]
    fn what_is_kept_is_bounded_and_the_least_used_goes_first() {
        let lines = |n: usize, len: u64| -> HashMap<Digest, Entry> {
            (0..n)
                .map(|i| {
                    let name = Digest::of(format!("picture {i}").as_bytes());
                    (Digest::of(format!("address {i}").as_bytes()), Entry { content: name, len, fetched: 1_000, used: 1_000 + i as u64 })
                })
                .collect()
        };
        let address = |i: usize| Digest::of(format!("address {i}").as_bytes());
        // Within both bounds: nothing goes.
        assert!(over_the_bounds(&lines(10, 100), 10, 1_000).is_empty());
        // One too many: the one used longest ago.
        assert_eq!(over_the_bounds(&lines(11, 10), 10, 1_000), [address(0)]);
        // Too heavy: as many of the oldest as it takes.
        assert_eq!(over_the_bounds(&lines(10, 100), 10, 750), [address(0), address(1), address(2)]);
        assert_eq!(over_the_bounds(&lines(3, 100), 10, 0).len(), 3);
        // A file two addresses share is counted once, and goes with its last address.
        let mut shared = lines(2, 600);
        let same = shared.values().next().unwrap().content;
        for entry in shared.values_mut() {
            entry.content = same;
        }
        assert!(over_the_bounds(&shared, 10, 600).is_empty());
        assert_eq!(over_the_bounds(&shared, 10, 599).len(), 2);

        // And for real, in a folder that keeps ten pictures and ten thousand bytes of them.
        let temp = Temp::new("bounded");
        let cache = temp.small(10, 10_000);
        let later = |seconds: u64| cache.ahead.fetch_add(seconds, Ordering::Relaxed);
        let url = |i: usize| format!("https://img.example/{i}.png");
        for i in 0..10 {
            assert!(cache.put(&url(i), &png(i as u8, 400)));
            later(10);
        }
        assert_eq!(cache.usage(), (10, 4_000));
        // The first is read again; then an eleventh comes, and the one used longest ago goes.
        assert!(cache.get(&url(0)).is_some());
        later(10);
        assert!(cache.put(&url(10), &png(10, 400)));
        assert_eq!(cache.usage(), (10, 4_000));
        assert!(cache.get(&url(0)).is_some() && cache.get(&url(1)).is_none() && cache.get(&url(2)).is_some() && cache.get(&url(10)).is_some());
        // A heavy one: as many of the oldest go as it takes to stay within the bytes.
        later(10);
        assert!(cache.put("https://img.example/heavy.png", &png(77, 7_000)));
        let (kept, bytes) = cache.usage();
        assert!(bytes <= 10_000 && kept == 8, "{kept} pictures, {bytes} bytes");
        assert!(cache.get("https://img.example/heavy.png").is_some() && cache.get(&url(3)).is_none() && cache.get(&url(10)).is_some());
        // One that is more than everything that may be kept is not kept at all.
        assert!(!cache.put("https://img.example/vast.png", &png(78, 10_001)));
        // No file is left of what went, and what is on disk is what is counted.
        cache.flush();
        let files = temp.files();
        assert_eq!(files.len(), kept + 1, "{files:?}");
        let on_disk: u64 = files.iter().filter(|f| *f != "index").map(|f| fs::metadata(temp.0.join(FOLDER).join(f)).unwrap().len()).sum();
        assert_eq!(on_disk, bytes);
        // The bounds hold at the next start too, whatever the index says.
        assert_eq!(temp.small(3, 10_000).usage().0, 3);
    }

    #[test]
    fn a_picture_is_kept_for_a_month_and_no_longer() {
        let temp = Temp::new("aged");
        let (fresh, old) = (png(5, 400), png(6, 400));
        {
            let cache = temp.cache();
            assert!(cache.put("https://img.example/fresh.png", &fresh) && cache.put("https://img.example/old.png", &old));
            cache.flush();
        }
        // The index, with one of them downloaded thirty-one days ago and used a minute ago.
        let index = temp.0.join(FOLDER).join("index");
        let aged: String = fs::read_to_string(&index)
            .unwrap()
            .lines()
            .map(|line| {
                let mut fields: Vec<String> = line.split(' ').map(str::to_string).collect();
                if fields[1] == Digest::of(&old).hex() {
                    fields[3] = (clock() - 31 * 86_400).to_string();
                    fields[4] = (clock() - 60).to_string();
                }
                fields.join(" ") + "\n"
            })
            .collect();
        fs::write(&index, aged).unwrap();
        let cache = temp.cache();
        assert_eq!(cache.usage(), (1, 400));
        assert_eq!(cache.get("https://img.example/old.png"), None);
        assert_eq!(cache.get("https://img.example/fresh.png").as_deref(), Some(&fresh[..]));
        // Its file is nothing's any more: it goes at the next tidying, once it has lain a while.
        assert_eq!(temp.files().len(), 3);
        cache.tidy();
        assert_eq!(temp.files().len(), 3, "a file just written is left alone: a download may be about to name it");
        let (_, orphan) = cache.path(&Digest::of(&old)).unwrap();
        let long_ago = SystemTime::now() - Duration::from_secs(3_600);
        fs::OpenOptions::new().write(true).open(&orphan).unwrap().set_modified(long_ago).unwrap();
        let (_, named) = cache.path(&Digest::of(&fresh)).unwrap();
        fs::OpenOptions::new().write(true).open(&named).unwrap().set_modified(long_ago).unwrap();
        cache.tidy();
        assert_eq!(temp.files().len(), 2);
        assert!(named.exists() && !orphan.exists());
        // A program left open for a month: what it kept at the start is asked for anew.
        cache.ahead.store(29 * 86_400, Ordering::Relaxed);
        assert!(cache.get("https://img.example/fresh.png").is_some());
        cache.ahead.store(31 * 86_400, Ordering::Relaxed);
        assert_eq!((cache.get("https://img.example/fresh.png"), cache.usage()), (None, (0, 0)));
    }

    #[test]
    fn a_folder_that_was_tampered_with_gives_nothing_it_should_not() {
        let temp = Temp::new("hostile");
        let picture = png(7, 900);
        let url = "https://img.example/a.png";
        let name = Digest::of(&picture).hex();
        let dir = temp.0.join(FOLDER);
        let file = dir.join(&name[..2]).join(&name);
        let fresh = || {
            let cache = temp.cache();
            assert!(cache.put(url, &picture));
            cache.flush();
            temp.cache()
        };
        // Other bytes of the same length under its name: not what the name says.
        let cache = fresh();
        let mut other = picture.clone();
        other[500] ^= 1;
        fs::write(&file, &other).unwrap();
        assert_eq!((cache.get(url), cache.usage()), (None, (0, 0)));
        // Longer, shorter, empty, gone.
        for tampered in [Some([&picture[..], b"<script>"].concat()), Some(picture[..400].to_vec()), Some(Vec::new()), None] {
            let cache = fresh();
            match &tampered {
                Some(bytes) => fs::write(&file, bytes).unwrap(),
                None => fs::remove_file(&file).unwrap(),
            }
            assert_eq!(cache.get(url), None, "{:?}", tampered.map(|bytes| bytes.len()));
            assert_eq!(cache.usage(), (0, 0));
        }
        // A folder where the file should be.
        let cache = fresh();
        fs::remove_file(&file).unwrap();
        fs::create_dir_all(&file).unwrap();
        assert_eq!(cache.get(url), None);
        fs::remove_dir_all(&file).unwrap();
        // An index that names a file somewhere else, by every trick a line can try: no line of
        // it is a line, so nothing is read, kept or removed on its say-so.
        let secret = temp.0.join("secret.txt");
        fs::write(&secret, &picture).unwrap();
        let address = Digest::of(url.as_bytes()).hex();
        let at = clock();
        let tricks = [
            format!("{address} ../../secret.txt 900 {at} {at}"),
            format!("{address} {} 900 {at} {at}", "../".repeat(21) + "a"),
            format!("{address} {} 900 {at} {at}", name.to_uppercase()),
            format!("{address} {name}/../../secret.txt 900 {at} {at}"),
            format!("{address} {name} -900 {at} {at}"),
            format!("{address} {name} 900 {at}"),
            format!("{address} {name} 900 {at} {at} extra"),
            format!("{address}  {name} 900 {at} {at}"),
            format!("{address}\t{name}\t900\t{at}\t{at}"),
            format!("{address} {name} 99999999999999999999999 {at} {at}"),
            format!("{address} {name} {} {at} {at}", MAX_PICTURE + 1),
            format!("{address} {name} 0 {at} {at}"),
            format!("{address} {name} 900 {} {at}", at + 10 * 86_400),
            format!("{address} {name} +900 {at} {at}"),
            format!("{} {name} 900 {at} {at}", &address[..63]),
            String::new(),
            "\u{0}\u{0}\u{0}".to_string(),
        ];
        for trick in &tricks {
            assert_eq!(parse_line(trick).filter(|(_, entry)| entry.fetched <= at + 86_400), None, "{trick:?}");
        }
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("index"), tricks.join("\n")).unwrap();
        let cache = temp.cache();
        assert_eq!((cache.usage(), cache.get(url)), ((0, 0), None));
        cache.clear().unwrap();
        assert_eq!(fs::read(&secret).unwrap(), picture);
        // An index that is not text, and one far too long, are no index.
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("index"), [0xff, 0xfe, 0x00, 0x80]).unwrap();
        assert_eq!(temp.cache().usage(), (0, 0));
        fs::write(dir.join("index"), "x".repeat(MAX_INDEX as usize + 1)).unwrap();
        assert_eq!(temp.cache().usage(), (0, 0));
        // More lines than pictures are kept: no more are taken in than that.
        let many: String =
            (0..MAX_PICTURES + 500).map(|i| format!("{} {name} 900 {at} {at}\n", Digest::of(format!("{i}").as_bytes()).hex())).collect();
        fs::write(dir.join("index"), many).unwrap();
        assert_eq!(temp.cache().usage().0, MAX_PICTURES);
    }

    #[test]
    fn junk_in_the_index_never_panics() {
        // The same numbers every run (xorshift): junk, and junk in the shape of a line.
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let at = clock();
        for round in 0..3_000 {
            let mut text = String::new();
            for _ in 0..next() % 6 {
                let field = |next: &mut dyn FnMut() -> u64| match next() % 6 {
                    0 => Digest::of(&next().to_le_bytes()).hex(),
                    1 => (next() % 100_000).to_string(),
                    2 => "f".repeat((next() % 70) as usize),
                    3 => String::from_utf8_lossy(&next().to_le_bytes()).into_owned(),
                    4 => at.to_string(),
                    _ => String::new(),
                };
                let line: Vec<String> = (0..next() % 7).map(|_| field(&mut next)).collect();
                text.push_str(&line.join(if round % 5 == 0 { "\t" } else { " " }));
                text.push(if round % 7 == 0 { '\r' } else { '\n' });
            }
            let entries = read_lines(&text, at, MAX_PICTURES);
            assert!(entries.len() <= MAX_PICTURES);
            for entry in entries.values() {
                assert!(entry.len > 0 && entry.len <= MAX_PICTURE as u64 && entry.fetched <= at + 86_400);
            }
            // What was read is written and read back as it was.
            assert_eq!(read_lines(&write_lines(&entries), at, MAX_PICTURES), entries);
        }
    }

    #[test]
    fn forgetting_one_and_clearing_all() {
        let temp = Temp::new("clear");
        let cache = temp.cache();
        let window: Box<dyn mailcraft_ui_egui::PictureCache> = Box::new(Kept(cache.clone()));
        for i in 0..6u8 {
            assert!(cache.put(&format!("https://img.example/{i}.png"), &png(i, 1_000)));
        }
        assert_eq!(window.usage(), (6, 6_000));
        // What the window found to be no picture, or the owner wants asked for afresh: its
        // file goes with it, and nothing else does.
        window.forget(" https://img.example/3.png ");
        window.forget("https://img.example/never-kept.png");
        assert_eq!((window.usage(), cache.get("https://img.example/3.png")), ((5, 5_000), None));
        assert_eq!(temp.files().len(), 6);
        // Things that are not ours, in the folder and beside it.
        let dir = temp.0.join(FOLDER);
        fs::write(dir.join("notes.txt"), "the owner's own").unwrap();
        fs::create_dir_all(dir.join("ab")).unwrap();
        fs::write(dir.join("ab").join("holiday.jpg"), "a picture somebody put here").unwrap();
        fs::write(temp.0.join("mailbox.mailcraft"), "{}").unwrap();
        cache.flush();
        assert!(temp.files().contains(&"index".to_string()));
        window.clear().unwrap();
        assert_eq!(window.usage(), (0, 0));
        assert_eq!(temp.files(), ["ab/holiday.jpg", "notes.txt"]);
        assert!(temp.0.join("mailbox.mailcraft").exists());
        for i in 0..6u8 {
            assert_eq!(cache.get(&format!("https://img.example/{i}.png")), None);
        }
        // Clearing a folder that was never there, and clearing twice, are nothing.
        let never = Cache::at(temp.0.join("never-made"));
        assert_eq!((never.clear(), never.clear(), never.usage()), (Ok(()), Ok(()), (0, 0)));
        assert!(!temp.0.join("never-made").exists());
        // It goes on being a cache afterwards.
        assert!(cache.put("https://img.example/after.png", &png(9, 300)));
        assert_eq!(window.usage(), (1, 300));
    }

    #[test]
    fn several_threads_keep_and_ask_at_once() {
        let temp = Temp::new("threads");
        let cache = temp.cache();
        let workers: Vec<_> = (0..8u8)
            .map(|worker| {
                let cache = cache.clone();
                std::thread::spawn(move || {
                    for i in 0..40u8 {
                        // Half of them the same pictures at the same addresses, from every thread.
                        let (url, bytes) = if i % 2 == 0 {
                            (format!("https://img.example/shared/{i}.png"), png(i, 2_000))
                        } else {
                            // (Each thread's own are of a length of their own: no two alike.)
                            (format!("https://img.example/{worker}/{i}.png"), png(i, 2_001 + usize::from(worker)))
                        };
                        assert!(cache.put(&url, &bytes));
                        assert!(cache.get(&url).is_some_and(|kept| kept == bytes), "{url} did not come back as it was kept");
                        // Now and then one of a thread's own is forgotten again.
                        if i % 7 == 0 && i % 2 == 1 {
                            cache.forget(&url);
                            assert!(cache.get(&url).is_none());
                        }
                        let _ = cache.usage();
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        cache.flush();
        let (count, bytes) = cache.usage();
        let again = temp.cache();
        assert_eq!(again.usage(), (count, bytes));
        // Twenty that every thread kept, and each thread's own twenty less the three it forgot.
        let own: u64 = (0..8u64).map(|worker| 17 * (2_001 + worker)).sum();
        assert_eq!((count, bytes), (20 + 8 * 17, 20 * 2_000 + own));
        // No half-written file is left, and every file is one the index names.
        assert!(temp.files().iter().all(|file| !file.ends_with(".part")), "{:?}", temp.files());
        let index = fs::read_to_string(temp.0.join(FOLDER).join("index")).unwrap();
        for file in temp.files().iter().filter(|file| *file != "index") {
            assert!(index.contains(file.rsplit('/').next().unwrap()), "{file} is named by nothing");
        }
    }

    #[test]
    fn the_host_asks_the_cache_before_the_network() {
        let temp = Temp::new("host");
        let cache = temp.cache();
        // An address the downloader refuses unseen (this machine): there is no network in this
        // test. With nothing kept the refusal comes back, in the downloader's words.
        let url = "http://127.0.0.1:9/logo.png";
        let refused = picture(url, Some(&cache)).unwrap_err();
        assert!(refused.contains("private network"), "{refused}");
        assert_eq!(picture(url, None).unwrap_err(), refused);
        // Kept, it comes from the folder, and nobody is asked.
        assert!(cache.put(url, &png(8, 700)));
        assert_eq!(picture(url, Some(&cache)), Ok(png(8, 700)));
        assert!(picture(url, None).is_err());
        cache.forget(url);
        assert!(picture(url, Some(&cache)).is_err());
    }
}
