// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The fonts installed on this computer, by the names of their families (#600): what a message's
//! `font-family` is looked up in. Nothing is fetched, nothing is installed and no font is bundled
//! for it; a message gets the fonts its reader already has.
//!
//! The fonts folders ([`crate::theme::font_dirs`]; on Windows the system's and the owner's own)
//! are walked once, on a thread of their own, and of each font file only its table directory and
//! its `name` and `OS/2` tables are read (a few kilobytes, never the outlines): the family names
//! in every language the file gives (name ids 1 and 16), the weight and whether the face is
//! italic. A face is read whole only when a message is to be drawn in it
//! (`reading::fonts`). Everything here is capped: the files looked at, the faces of a collection,
//! the size of a table, the names kept. The browser has no fonts folder: there the index is empty.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::fs::File;
#[cfg(not(target_arch = "wasm32"))]
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The most font files that are looked at.
const MAX_FILES: usize = 10_000;
/// The most faces of one collection (`.ttc`) that are looked at.
const MAX_COLLECTION: u32 = 64;
/// The most tables a face's directory may list.
const MAX_TABLES: u16 = 512;
/// The largest `name` table that is read: a font with names in many languages has a few dozen
/// kilobytes of them.
const MAX_NAME_TABLE: u32 = 512 * 1024;
/// The most family names kept, and the most faces kept for one of them.
const MAX_FAMILIES: usize = 20_000;
const MAX_FACES_A_FAMILY: usize = 64;
/// The longest family name kept, in characters.
const MAX_NAME: usize = 64;

/// `usWidthClass` of a face that is neither condensed nor expanded.
const NORMAL_WIDTH: u16 = 5;

/// One installed face.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Face {
    pub path: Arc<Path>,
    /// Which face of the file: 0, or the place in a collection.
    pub index: u32,
    /// The weight, 1 to 1000 (400 is regular, 700 bold).
    pub weight: u16,
    /// The width, 1 (ultra-condensed) to 9 (ultra-expanded); 5 is normal.
    pub width: u16,
    pub italic: bool,
}

/// The installed faces by family name, lower case.
#[derive(Debug, Default)]
pub struct FontIndex {
    families: HashMap<String, Vec<Face>>,
}

impl FontIndex {
    /// The fonts installed on this system. Slow (it reads a little of every font file): call it
    /// off the window's thread.
    pub fn system() -> FontIndex {
        FontIndex::of_dirs(&system_dirs())
    }

    /// The fonts in `dirs` and the folders inside them.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn of_dirs(dirs: &[PathBuf]) -> FontIndex {
        let mut index = FontIndex::default();
        for path in font_files(dirs) {
            for (names, face) in faces_of(&path) {
                index.add(names, face);
            }
        }
        index
    }

    /// The browser has no fonts folders.
    #[cfg(target_arch = "wasm32")]
    pub fn of_dirs(_dirs: &[PathBuf]) -> FontIndex {
        FontIndex::default()
    }

    fn add(&mut self, names: Vec<String>, face: Face) {
        for name in names {
            if !self.families.contains_key(&name) && self.families.len() >= MAX_FAMILIES {
                continue;
            }
            let faces = self.families.entry(name).or_default();
            // The first file of a face is kept (the owner's own folders come first).
            let known = faces.iter().any(|f| f.weight == face.weight && f.width == face.width && f.italic == face.italic);
            if !known && faces.len() < MAX_FACES_A_FAMILY {
                faces.push(face.clone());
            }
        }
    }

    /// Whether a family of that name (lower case) is installed.
    pub fn has(&self, family: &str) -> bool {
        self.families.contains_key(family)
    }

    /// How many family names are known.
    pub fn len(&self) -> usize {
        self.families.len()
    }

    /// Whether no font was found.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.families.is_empty()
    }

    /// The face of `family` (lower case) to draw text in: for regular text the face nearest to
    /// weight 400 (a family of heavy faces alone, Arial Black, has its own), for bold text a face
    /// of weight 600 or more, nearest to 700; of normal width before condensed or expanded (a
    /// family's condensed faces often share its name), and upright before italic. `None` when the
    /// family has no such face: bold text in a family without a bold face.
    pub fn face(&self, family: &str, bold: bool) -> Option<&Face> {
        let faces = self.families.get(family)?;
        let target: i32 = if bold { 700 } else { 400 };
        faces
            .iter()
            .filter(|f| !bold || f.weight >= 600)
            .min_by_key(|f| ((i32::from(f.width) - i32::from(NORMAL_WIDTH)).abs(), f.italic, (i32::from(f.weight) - target).abs()))
    }
}

/// The folders this system keeps its fonts in.
#[cfg(windows)]
fn system_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    // What the owner installed for himself alone (Install, rather than Install for all users).
    if let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|d| !d.is_empty()) {
        dirs.push(PathBuf::from(local).join("Microsoft").join("Windows").join("Fonts"));
    }
    if let Some(root) = ["WINDIR", "SystemRoot"].into_iter().filter_map(std::env::var_os).find(|r| !r.is_empty()) {
        dirs.push(PathBuf::from(root).join("Fonts"));
    }
    dirs
}

#[cfg(all(not(windows), not(target_arch = "wasm32")))]
fn system_dirs() -> Vec<PathBuf> {
    crate::theme::font_dirs()
}

/// The browser has no fonts folder.
#[cfg(target_arch = "wasm32")]
fn system_dirs() -> Vec<PathBuf> {
    Vec::new()
}

/// The font files (`.ttf`, `.otf`, `.ttc`, `.otc`) in `dirs` and the folders in them, as
/// [`crate::theme`] walks them for its own faces: so deep, so many entries a folder, no link to
/// a folder followed, at most [`MAX_FILES`] files.
#[cfg(not(target_arch = "wasm32"))]
fn font_files(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    for root in dirs {
        let mut entries = 0usize;
        let mut pending: Vec<(PathBuf, usize)> = vec![(root.clone(), 0)];
        'walk: while let Some((dir, depth)) = pending.pop() {
            let Ok(list) = std::fs::read_dir(&dir) else { continue };
            let mut inner = Vec::new();
            let mut here = Vec::new();
            for entry in list.flatten() {
                entries = entries.saturating_add(1);
                if entries > crate::theme::FONT_DIR_ENTRIES {
                    break 'walk;
                }
                let Ok(kind) = entry.file_type() else { continue };
                if kind.is_dir() {
                    if depth < crate::theme::FONT_DIR_DEPTH {
                        inner.push((entry.path(), depth.saturating_add(1)));
                    }
                    continue;
                }
                let path = entry.path();
                let font = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc" | "otc"));
                // A plain file, where a link leads (opening anything else, a pipe, could wait forever).
                if font && std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
                    here.push(path);
                }
            }
            here.sort();
            for path in here {
                if files.len() >= MAX_FILES {
                    return files;
                }
                files.push(path);
            }
            inner.sort();
            pending.extend(inner.into_iter().rev());
        }
    }
    files
}

/// `len` bytes of `file` from `at`, all of them or nothing.
#[cfg(not(target_arch = "wasm32"))]
fn read_at(file: &mut File, at: u64, len: usize) -> Option<Vec<u8>> {
    file.seek(SeekFrom::Start(at)).ok()?;
    let mut bytes = vec![0; len];
    file.read_exact(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(not(target_arch = "wasm32"))]
fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    bytes.get(at..at.checked_add(2)?).and_then(|b| <[u8; 2]>::try_from(b).ok()).map(u16::from_be_bytes)
}

#[cfg(not(target_arch = "wasm32"))]
fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    bytes.get(at..at.checked_add(4)?).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_be_bytes)
}

/// The faces of the font file at `path`, each with its family names; none when it is not a font
/// or can't be read.
#[cfg(not(target_arch = "wasm32"))]
fn faces_of(path: &Path) -> Vec<(Vec<String>, Face)> {
    let Ok(mut file) = File::open(path) else { return Vec::new() };
    let Some(head) = read_at(&mut file, 0, 12) else { return Vec::new() };
    let starts: Vec<u32> = if u32_at(&head, 0) == Some(0x7474_6366) {
        // A collection: how many faces, then where each begins.
        let count = u32_at(&head, 8).unwrap_or(0).min(MAX_COLLECTION);
        let Some(places) = read_at(&mut file, 12, (count as usize).saturating_mul(4)) else { return Vec::new() };
        (0..count as usize).filter_map(|i| u32_at(&places, i.saturating_mul(4))).collect()
    } else {
        vec![0]
    };
    let mut faces = Vec::new();
    // One path for every face and name of the file.
    let shared: Arc<Path> = Arc::from(path);
    for (index, start) in starts.into_iter().enumerate() {
        let Ok(index) = u32::try_from(index) else { break };
        if let Some((names, (weight, width, italic))) = face_at(&mut file, u64::from(start)) {
            faces.push((names, Face { path: shared.clone(), index, weight, width, italic }));
        }
    }
    faces
}

/// The family names, and the weight, width and slant, of the face whose directory starts at `start`.
#[cfg(not(target_arch = "wasm32"))]
fn face_at(file: &mut File, start: u64) -> Option<(Vec<String>, (u16, u16, bool))> {
    let head = read_at(file, start, 12)?;
    // TrueType outlines, `OTTO` (CFF outlines) or Apple's `true`: what the font reader takes.
    if !matches!(u32_at(&head, 0), Some(0x0001_0000 | 0x4F54_544F | 0x7472_7565)) {
        return None;
    }
    let tables = u16_at(&head, 4)?.min(MAX_TABLES);
    let directory = read_at(file, start.checked_add(12)?, usize::from(tables).saturating_mul(16))?;
    let mut name = None;
    let mut os2 = None;
    let mut head_table = None;
    for i in 0..usize::from(tables) {
        let record = i.saturating_mul(16);
        let tag = directory.get(record..record.saturating_add(4))?;
        let place = (u32_at(&directory, record.saturating_add(8))?, u32_at(&directory, record.saturating_add(12))?);
        match tag {
            b"name" => name = Some(place),
            b"OS/2" => os2 = Some(place),
            b"head" => head_table = Some(place),
            _ => {}
        }
    }
    let (at, len) = name?;
    if len == 0 || len > MAX_NAME_TABLE {
        return None;
    }
    let names = family_names(&read_at(file, u64::from(at), len as usize)?);
    if names.is_empty() {
        return None;
    }
    // The weight, width and slant from `OS/2` (`usWeightClass`, `usWidthClass`, `fsSelection`),
    // else the weight and slant from `head` (`macStyle`).
    let os2 = os2.filter(|(_, len)| *len >= 64).and_then(|(at, _)| read_at(file, u64::from(at), 64));
    let (weight, width, italic) = match os2 {
        Some(t) => (
            u16_at(&t, 4).filter(|w| (1..=1000).contains(w)).unwrap_or(400),
            u16_at(&t, 6).filter(|w| (1..=9).contains(w)).unwrap_or(NORMAL_WIDTH),
            u16_at(&t, 62).is_some_and(|s| s & 1 != 0),
        ),
        None => {
            let style = head_table
                .filter(|(_, len)| *len >= 46)
                .and_then(|(at, _)| read_at(file, u64::from(at), 46))
                .and_then(|t| u16_at(&t, 44))
                .unwrap_or(0);
            (if style & 1 != 0 { 700 } else { 400 }, NORMAL_WIDTH, style & 2 != 0)
        }
    };
    Some((names, (weight, width, italic)))
}

/// The family names a `name` table gives (ids 1 and 16), in every language it has them in, lower
/// case and trimmed, each once.
#[cfg(not(target_arch = "wasm32"))]
fn family_names(table: &[u8]) -> Vec<String> {
    use skrifa::raw::tables::name::Name;
    use skrifa::raw::types::NameId;
    use skrifa::raw::{FontData, FontRead};
    let Ok(name) = Name::read(FontData::new(table)) else { return Vec::new() };
    let data = name.string_data();
    let mut names: Vec<String> = Vec::new();
    for record in name.name_record() {
        let id = record.name_id();
        if id != NameId::FAMILY_NAME && id != NameId::TYPOGRAPHIC_FAMILY_NAME {
            continue;
        }
        // Unicode, or the Macintosh's own encoding in English.
        let readable = record.is_unicode() || (record.platform_id() == 1 && record.encoding_id() == 0 && record.language_id() == 0);
        if !readable {
            continue;
        }
        let Ok(text) = record.string(data) else { continue };
        let text: String = text.chars().take(MAX_NAME.saturating_add(1)).collect();
        let text = text.trim().to_lowercase();
        if text.is_empty() || text.chars().count() > MAX_NAME || text.chars().any(char::is_control) || names.contains(&text) {
            continue;
        }
        names.push(text);
    }
    names
}

#[cfg(test)]
#[path = "font_index_tests.rs"]
pub(crate) mod font_index_tests;
