// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Header blocks packed (#1376): the header lines of a message that nothing reads by name
//! ([`mailcraft_model::HeaderBlock`]) are kept, when they are more than the record holds, in a
//! few pack files in the content folder rather than in one content file each, which on a disk of
//! 4 KB pieces cost twice their size and a file apiece.
//!
//! ```text
//! mailbox.mailcraft.blobs/
//!     headers-<n>.pack     header blocks one after another, nothing between them
//! ```
//!
//! - **Named by the record:** a message's record names its block `[SHA-256, length, pack,
//!   offset]` (`"headers"` in its content names); a block a development build of this format
//!   wrote as a content file of its own is named `[SHA-256, length]` and read from there, until
//!   the next whole rewrite of the mailbox file moves it into a pack.
//! - **Written before it is named:** a save appends the blocks it writes to the pack and flushes
//!   it before the mailbox file that names them is written. A pack is only ever appended to: bytes
//!   a save cut short wrote at its end are named by nothing, and the next save writes after them.
//! - **Read as hostile:** what the record says is checked, with arithmetic that can't overflow, against
//!   the pack's length and [`MAX_HEADER_BLOCK`], and what is read against the SHA-256 it names; a
//!   pack that is missing, short or damaged leaves the block unavailable, never the mailbox closed.
//! - **Space given back:** the whole rewrite of the mailbox file writes a fresh pack of the blocks
//!   it names when the one it has holds more than [`PACK_SLACK`] that nothing names, under a new
//!   number, flushed before the file naming it is renamed over the old one; the old pack goes
//!   after. A save that finds its pack mostly dead writes the mailbox file whole for this.

use std::collections::HashMap;
use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use mailcraft_model::headers::MAX_HEADER_BLOCK;

use super::{Digest, MAX_ENTRIES, Ref, get_loose, lock, sync_dir_of};

/// What a pack may hold that no mailbox file names before the next whole rewrite writes a fresh
/// one (16 MB; less in tests, so that small mailboxes get there).
pub(super) const PACK_SLACK: u64 = if cfg!(test) { 16 << 10 } else { 16 << 20 };

/// Where a header block is: which pack, and how far into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Loc {
    pub(super) pack: u32,
    pub(super) at: u64,
}

/// The name of pack `n` (numbers start at 1).
pub(super) fn name(n: u32) -> Option<String> {
    (n > 0).then(|| format!("headers-{n}.pack"))
}

/// The number of a pack from its file name, and nothing for any other name: digits without a
/// leading zero that fit 32 bits.
pub(super) fn number(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("headers-")?.strip_suffix(".pack")?;
    if digits.is_empty() || digits.len() > 10 || digits.starts_with('0') || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u32>().ok().filter(|n| *n > 0)
}

/// The packs in the content folder `dir` (plain files of a pack's name), at most
/// [`MAX_ENTRIES`] entries looked at.
pub(super) fn on_disk(dir: &Path) -> Vec<(u32, PathBuf)> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .take(MAX_ENTRIES)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().to_str().and_then(number).map(|n| (n, e.path())))
        .collect()
}

/// What is known of the packs of one content folder: the one appended to, where each block named
/// is, and whether an opening wrote into it without flushing it.
#[derive(Default)]
struct Known {
    current: Option<u32>,
    at: HashMap<Digest, Loc>,
    unsynced: bool,
}

/// Per content folder, so that what reads a block ([`super::get`]) needs nothing but the folder.
fn known() -> &'static Mutex<HashMap<PathBuf, Known>> {
    static KNOWN: OnceLock<Mutex<HashMap<PathBuf, Known>>> = OnceLock::new();
    KNOWN.get_or_init(Default::default)
}

/// What a mailbox file just read says of the packs of `dir`: where its blocks are (`at`), the
/// pack to append to, and whether an opening wrote into it unflushed.
pub(super) fn install(dir: &Path, current: Option<u32>, at: HashMap<Digest, Loc>, unsynced: bool) {
    lock(known()).insert(dir.to_path_buf(), Known { current, at, unsynced });
}

/// What a mailbox file somebody else wrote since it was read here says of the packs of `dir`
/// (`at`, the highest pack it names `named`): taken over what was known, which may name packs
/// that writer has since given up. What is known here and the file doesn't name stays (mail in
/// memory may still have its block there); every place is checked against the pack before it is
/// named again ([`Writer::place`]).
pub(super) fn refresh(dir: &Path, named: Option<u32>, at: HashMap<Digest, Loc>) {
    let mut all = lock(known());
    let k = all.entry(dir.to_path_buf()).or_default();
    k.at.extend(at);
    // A pack an opening wrote into unflushed, whose places are still known, is flushed before it
    // stops being the one a save flushes ([`Writer::sync`]); one that can't be stays that one.
    if k.unsynced {
        if let Some(n) = k.current
            && sync_pack(dir, n).is_err()
        {
            return;
        }
        k.unsynced = false;
    }
    // Appended to from now on: the pack the file names last, else the one known if it is there.
    k.current = named.or(k.current.filter(|n| len(dir, *n) > 0));
}

/// Flush pack `n` of `dir` and the folder it is in. One that is gone is no error: what it held is
/// unavailable, which the reads say, and a save that failed for it would fail every time.
fn sync_pack(dir: &Path, n: u32) -> std::io::Result<()> {
    let Some(file) = name(n).map(|name| dir.join(name)) else { return Ok(()) };
    match fs::OpenOptions::new().append(true).open(&file) {
        Ok(f) => f.sync_data()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            log::warn!("{}: a pack of header lines is gone", dir.display());
            return Ok(());
        }
        Err(e) => return Err(e),
    }
    sync_dir_of(&file);
    Ok(())
}

/// Whether a pack of `dir` an opening wrote into is still to be flushed.
#[cfg(test)]
pub(super) fn unsynced(dir: &Path) -> bool {
    lock(known()).get(dir).is_some_and(|k| k.unsynced)
}

/// Nothing is known of the packs of `dir` (the file there is of a form that names none).
pub(super) fn forget(dir: &Path) {
    lock(known()).remove(dir);
}

/// Where the block `hash` of `dir` is, as far as is known.
pub(super) fn find(dir: &Path, hash: &Digest) -> Option<Loc> {
    lock(known()).get(dir).and_then(|k| k.at.get(hash).copied())
}

/// The pack of `dir` a save appends to.
pub(super) fn current(dir: &Path) -> Option<u32> {
    lock(known()).get(dir).and_then(|k| k.current)
}

/// How long pack `n` of `dir` is (0 when it isn't there).
pub(super) fn len(dir: &Path, n: u32) -> u64 {
    name(n).and_then(|name| fs::symlink_metadata(dir.join(name)).ok()).filter(|m| m.file_type().is_file()).map_or(0, |m| m.len())
}

/// The block `r` of `dir`, from the pack it is known to be in.
pub(super) fn read(dir: &Path, r: &Ref) -> Option<Vec<u8>> {
    read_at(dir, find(dir, &r.hash)?, r)
}

/// The block `r` from `loc` in the packs of `dir`, if it is there and is what its name says: a
/// plain file (never a link), long enough, and bytes of that length with that SHA-256. Every
/// number here came from a file anybody could have written.
pub(super) fn read_at(dir: &Path, loc: Loc, r: &Ref) -> Option<Vec<u8>> {
    let mut file = name(loc.pack).and_then(|name| open_pack(&dir.join(name)).ok().flatten())?;
    read_from(&mut file, loc, r).ok().flatten()
}

/// A pack opened for reading, and its length: `None` when there is no plain file of that name
/// (never a link). An error is one of reading it now (no permission, a file in use, the disk),
/// not of its not being there.
fn open_pack(file: &Path) -> std::io::Result<Option<(fs::File, u64)>> {
    let gone = |e: std::io::Error| if e.kind() == std::io::ErrorKind::NotFound { Ok(None) } else { Err(e) };
    #[cfg(windows)]
    let opened = {
        use std::os::windows::fs::OpenOptionsExt as _;
        // FILE_FLAG_OPEN_REPARSE_POINT: a link opens as itself, and then isn't a plain file.
        match fs::OpenOptions::new().read(true).custom_flags(0x0020_0000).open(file) {
            Ok(f) => f,
            Err(e) => return gone(e),
        }
    };
    #[cfg(not(windows))]
    let opened = {
        match fs::symlink_metadata(file) {
            Ok(m) if m.file_type().is_file() => {}
            Ok(_) => return Ok(None),
            Err(e) => return gone(e),
        }
        match fs::File::open(file) {
            Ok(f) => f,
            Err(e) => return gone(e),
        }
    };
    let meta = opened.metadata()?;
    Ok(meta.file_type().is_file().then_some((opened, meta.len())))
}

/// The block `r` from `loc` in the open pack `file` (of that length): `None` when it isn't
/// there or isn't what its name says, an error when it couldn't be read.
fn read_from(file: &mut (fs::File, u64), loc: Loc, r: &Ref) -> std::io::Result<Option<Vec<u8>>> {
    let (file, on_disk) = file;
    let Ok(len) = usize::try_from(r.len) else { return Ok(None) };
    if r.len > MAX_HEADER_BLOCK as u64 || loc.at.checked_add(r.len).is_none_or(|end| end > *on_disk) {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(loc.at))?;
    let mut bytes = Vec::new();
    if bytes.try_reserve_exact(len).is_err() {
        return Ok(None);
    }
    std::io::Read::by_ref(file).take(r.len).read_to_end(&mut bytes)?;
    Ok((bytes.len() == len && Digest::of(&bytes) == r.hash).then_some(bytes))
}

/// Write zeros over the block `r` where it is in the packs of `dir`, once the bytes there are
/// checked to be it, and forget where it was: for a block nothing names any more whose lines are
/// to leave the disk now (the mail of an account removed), not at the next fresh pack. Whether it
/// was written over.
/// The blocks are taken a pack at a time, through one handle and one flush for each (an account
/// of tens of thousands of messages removed is not as many flushes). The blocks written over.
pub(super) fn wipe(dir: &Path, refs: &[Ref]) -> Vec<Ref> {
    let mut by_pack: HashMap<u32, Vec<(Loc, Ref)>> = HashMap::new();
    {
        let known = lock(known());
        let Some(k) = known.get(dir) else { return Vec::new() };
        for r in refs {
            if let Some(loc) = k.at.get(&r.hash) {
                by_pack.entry(loc.pack).or_default().push((*loc, *r));
            }
        }
    }
    let mut wiped = Vec::new();
    for (n, blocks) in by_pack {
        let Some(path) = name(n).map(|name| dir.join(name)) else { continue };
        // A plain file only, never a link; read and written through the one handle.
        if !fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) {
            continue;
        }
        let Ok(f) = fs::OpenOptions::new().read(true).write(true).open(&path) else { continue };
        let Ok(on_disk) = f.metadata().map(|m| m.len()) else { continue };
        let mut file = (f, on_disk);
        let mut here = Vec::new();
        for (loc, r) in blocks {
            // Only bytes checked to be the block: a stale place never has something else zeroed.
            if !matches!(read_from(&mut file, loc, &r), Ok(Some(_))) {
                continue;
            }
            let Ok(len) = usize::try_from(r.len) else { continue };
            let zeroed = file.0.seek(SeekFrom::Start(loc.at)).and_then(|_| file.0.write_all(&vec![0u8; len]));
            if zeroed.is_ok() {
                here.push(r);
            }
        }
        if !here.is_empty() && file.0.sync_data().is_ok() {
            wiped.extend(here);
        }
    }
    if let Some(k) = lock(known()).get_mut(dir) {
        for r in &wiped {
            k.at.remove(&r.hash);
        }
    }
    wiped
}

/// Remove pack `n` of `dir` when it is a plain file. Whether it went.
pub(super) fn remove(dir: &Path, n: u32) -> bool {
    let Some(file) = name(n).map(|name| dir.join(name)) else { return false };
    fs::symlink_metadata(&file).is_ok_and(|m| m.file_type().is_file()) && fs::remove_file(&file).is_ok()
}

/// Writes the header blocks of one save (or of one opening of a file of an older format) into a
/// pack of a content folder, and says where each went.
pub(super) struct Writer {
    dir: PathBuf,
    /// The pack written to: `None` until the first block when a new one is to be made.
    pack: Option<u32>,
    file: Option<fs::File>,
    /// The pack was made by this writer: the folder is flushed with it.
    created: bool,
    /// A pack of its own, of only the blocks placed here (a whole rewrite giving space back):
    /// what the packs there already hold is read, never named.
    fresh: bool,
    /// Blocks that aren't in memory are brought in from where they are (a whole rewrite).
    migrate: bool,
    placed: HashMap<Digest, Loc>,
    /// How long each pack named by what is known was when this writer first looked: a place there
    /// that goes past it isn't named again.
    lens: HashMap<u32, u64>,
    /// The pack blocks are brought in from, kept open from one to the next.
    reader: Option<(u32, (fs::File, u64))>,
    /// Blocks read from content files of their own, to remove once a pack holds them.
    loose: Vec<Ref>,
    /// The pack an opening wrote into unflushed was flushed by [`Writer::sync`].
    synced_known: bool,
    /// Bytes written.
    pub(super) wrote: u64,
}

impl Writer {
    fn new(dir: &Path, pack: Option<u32>, fresh: bool, migrate: bool) -> Writer {
        Writer {
            dir: dir.to_path_buf(),
            pack,
            file: None,
            created: false,
            fresh,
            migrate,
            placed: HashMap::new(),
            lens: HashMap::new(),
            reader: None,
            loose: Vec::new(),
            synced_known: false,
            wrote: 0,
        }
    }

    /// For a save appended to the mailbox file: blocks in memory go at the end of the pack in use.
    pub(super) fn appending(dir: &Path) -> Writer {
        Writer::new(dir, current(dir), false, false)
    }

    /// For an opening of a file of an older format: its blocks go into a pack of their own, so that
    /// nothing named before is ever written after.
    pub(super) fn stashing(dir: &Path) -> Writer {
        Writer::new(dir, None, false, false)
    }

    /// For a whole rewrite: every block named goes into a pack, read from where it is; into a
    /// fresh one when `fresh`.
    pub(super) fn compacting(dir: &Path, fresh: bool) -> Writer {
        Writer::new(dir, if fresh { None } else { current(dir) }, fresh, true)
    }

    /// Where the block `r` is to be named: where it is in a pack already, or where it is written
    /// now from `held` (its bytes in memory) or, when this writer brings blocks in, from where it
    /// is. `None`: it stays named as a content file of its own (the block isn't in memory and
    /// isn't in a pack, or can't be read: it is then unavailable, and kept named all the same).
    ///
    /// A place known from before is named again only when its pack is there and long enough for
    /// it (another writer of the file may have given that pack up). A writer of a fresh pack fails
    /// when a block known to be in a pack can't be read now (no permission, a file in use, the
    /// disk): the pack it is in goes once the fresh one is named, and the block with it.
    pub(super) fn place(&mut self, r: &Ref, held: Option<&[u8]>) -> std::io::Result<Option<Loc>> {
        if let Some(loc) = self.placed.get(&r.hash) {
            return Ok(Some(*loc));
        }
        if r.len > MAX_HEADER_BLOCK as u64 {
            return Ok(None);
        }
        let known = find(&self.dir, &r.hash);
        if !self.fresh
            && let Some(loc) = known
            && self.holds(loc, r)
        {
            return Ok(Some(loc));
        }
        let read: Vec<u8>;
        let bytes = match held {
            Some(bytes) => bytes,
            None if self.migrate => {
                let packed = match known {
                    Some(loc) => match self.read_old(loc, r) {
                        Ok(bytes) => bytes,
                        Err(e) if self.fresh => return Err(e),
                        Err(_) => None,
                    },
                    None => None,
                };
                read = match packed {
                    Some(bytes) => bytes,
                    None => match get_loose(&self.dir, r) {
                        Some(bytes) => {
                            self.loose.push(*r);
                            bytes
                        }
                        None => return Ok(None),
                    },
                };
                read.as_slice()
            }
            None => return Ok(None),
        };
        if bytes.len() as u64 != r.len {
            return Ok(None);
        }
        let loc = self.append(bytes)?;
        self.placed.insert(r.hash, loc);
        Ok(Some(loc))
    }

    /// Whether pack `loc.pack` is long enough to hold the block `r` at `loc.at`, by its length when
    /// this writer first looked.
    fn holds(&mut self, loc: Loc, r: &Ref) -> bool {
        let on_disk = *self.lens.entry(loc.pack).or_insert_with(|| len(&self.dir, loc.pack));
        loc.at.checked_add(r.len).is_some_and(|end| end <= on_disk)
    }

    /// The block `r` from `loc`, through a handle kept open for the next block of the same pack.
    fn read_old(&mut self, loc: Loc, r: &Ref) -> std::io::Result<Option<Vec<u8>>> {
        if self.reader.as_ref().is_none_or(|(n, _)| *n != loc.pack) {
            let Some(file) = name(loc.pack).map(|name| self.dir.join(name)) else { return Ok(None) };
            self.reader = open_pack(&file)?.map(|f| (loc.pack, f));
        }
        match &mut self.reader {
            Some((_, file)) => read_from(file, loc, r),
            None => Ok(None),
        }
    }

    /// The pack's file, opened (or made) for appending.
    fn open(&mut self) -> std::io::Result<u32> {
        if let (Some(n), Some(_)) = (self.pack, &self.file) {
            return Ok(n);
        }
        if let Err(e) = super::make_dir(&self.dir)
            && !self.dir.is_dir()
        {
            return Err(e);
        }
        if let Some(n) = self.pack {
            let file = self.dir.join(name(n).ok_or_else(|| std::io::Error::other("no such pack"))?);
            // Never written through a link, or into anything but a plain file.
            let there = match fs::symlink_metadata(&file) {
                Ok(m) if m.file_type().is_file() => true,
                Ok(_) => return Err(std::io::Error::other("something that isn't a pack has its name")),
                Err(_) => false,
            };
            let mut open = fs::OpenOptions::new();
            open.read(true).append(true).create(!there);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
            let f = open.open(&file)?;
            self.created = !there;
            self.file = Some(f);
            return Ok(n);
        }
        // A new pack, under a number nothing has used: never one a save cut short left behind.
        // (After the highest number in use; from the lowest free one when a file named the highest
        // there can be, so that a hostile name never stops every save.)
        let used: Vec<u32> = on_disk(&self.dir).into_iter().map(|(n, _)| n).chain(current(&self.dir)).collect();
        let mut n = used.iter().copied().max().filter(|n| *n < u32::MAX - 8).unwrap_or(0);
        for _ in 0..64 {
            n = n.checked_add(1).ok_or_else(|| std::io::Error::other("the content folder has too many packs"))?;
            if used.contains(&n) {
                continue;
            }
            let file = self.dir.join(name(n).ok_or_else(|| std::io::Error::other("no such pack"))?);
            let mut open = fs::OpenOptions::new();
            open.read(true).append(true).create_new(true);
            // Mail is private: nobody else on the machine reads it.
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
            match open.open(&file) {
                Ok(f) => {
                    (self.pack, self.file, self.created) = (Some(n), Some(f), true);
                    return Ok(n);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::other("no name for a new pack"))
    }

    /// Write `bytes` at the end of the pack; where they went. Where the end is, is asked of the
    /// file itself, and what is there afterwards read back: a program without the window's lock
    /// (the command line, an agent) appending to the same pack at the same moment makes this save
    /// fail, never a mailbox file name the wrong bytes.
    fn append(&mut self, bytes: &[u8]) -> std::io::Result<Loc> {
        let pack = self.open()?;
        let len = bytes.len() as u64;
        let file = self.file.as_mut().ok_or_else(|| std::io::Error::other("the pack isn't open"))?;
        let at = file.seek(SeekFrom::End(0))?;
        at.checked_add(len).ok_or_else(|| std::io::Error::other("the pack is full"))?;
        // (What was written of a block that failed is named by nothing; the next goes after it.)
        file.write_all(bytes)?;
        let on_disk = file.metadata()?.len();
        let mut there = (file.try_clone()?, on_disk);
        let r = Ref { hash: Digest::of(bytes), len };
        if read_from(&mut there, Loc { pack, at }, &r)?.is_none() {
            return Err(std::io::Error::other("the pack was written to by another program at the same time"));
        }
        self.wrote = self.wrote.saturating_add(len);
        Ok(Loc { pack, at })
    }

    /// Flush what was written, and what an opening wrote into the pack in use unflushed, before a
    /// mailbox file that names it is written.
    pub(super) fn sync(&mut self) -> std::io::Result<()> {
        if let Some(f) = &self.file {
            f.sync_data()?;
            if self.created {
                sync_dir_of(&self.dir.join("x"));
            }
        }
        let pending = {
            let known = lock(known());
            known.get(&self.dir).filter(|k| k.unsynced).and_then(|k| k.current)
        };
        if let Some(n) = pending.filter(|n| !self.fresh && Some(*n) != self.pack.filter(|_| self.file.is_some())) {
            sync_pack(&self.dir, n)?;
        }
        self.synced_known = pending.is_some() && !self.fresh;
        Ok(())
    }

    /// The pack written to.
    pub(super) fn pack(&self) -> Option<u32> {
        self.pack
    }

    /// The mailbox file that names what was placed is on disk: what was written is known, for
    /// reading and placing again. Returns the pack given up (a fresh pack took its place) and the
    /// blocks moved into a pack from content files of their own.
    pub(super) fn done(&mut self) -> (Option<u32>, Vec<Ref>) {
        let (placed, loose) = (std::mem::take(&mut self.placed), std::mem::take(&mut self.loose));
        let mut all = lock(known());
        let k = all.entry(self.dir.clone()).or_default();
        if self.fresh {
            let old = k.current.filter(|n| Some(*n) != self.pack);
            *k = Known { current: self.pack, at: placed, unsynced: false };
            return (old, loose);
        }
        if self.synced_known {
            k.unsynced = false;
        }
        if self.pack.is_some() && self.file.is_some() {
            k.current = self.pack;
        }
        for (hash, loc) in placed {
            k.at.entry(hash).or_insert(loc);
        }
        (None, loose)
    }

    /// It writes a pack of its own ([`Writer::compacting`]).
    pub(super) fn is_fresh(&self) -> bool {
        self.fresh
    }

    /// An opening's writer, its blocks known at once (they are named on the messages before any
    /// save), unflushed until the save that first names them.
    pub(super) fn install_stashed(self, mut at: HashMap<Digest, Loc>, named: Option<u32>) {
        let unsynced = self.wrote > 0;
        let current = if unsynced { self.pack } else { named };
        for (hash, loc) in self.placed {
            at.entry(hash).or_insert(loc);
        }
        install(&self.dir, current, at, unsynced);
    }
}
