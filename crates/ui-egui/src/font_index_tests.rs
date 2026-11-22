// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #600: the installed fonts are found by the names inside them, whatever their files are called,
//! and nothing that isn't a font gets in.

use std::path::PathBuf;

use super::{FontIndex, MAX_COLLECTION, family_names};

const REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
const SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");
const MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

/// A folder to stand in for a fonts folder, removed when dropped.
pub(crate) struct FontsDir(pub PathBuf);

impl FontsDir {
    pub(crate) fn new(name: &str) -> FontsDir {
        let dir = std::env::temp_dir().join(format!("mailcraft-font-index-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        FontsDir(dir)
    }

    pub(crate) fn put(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, bytes).unwrap();
        path
    }

    pub(crate) fn index(&self) -> FontIndex {
        FontIndex::of_dirs(std::slice::from_ref(&self.0))
    }
}

impl Drop for FontsDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `font` as a collection of `faces` faces, all of them the one face: its directory moved past the
/// collection's header, every table's offset moved with it.
pub(crate) fn collection(font: &[u8], faces: u32) -> Vec<u8> {
    let header = 12 + 4 * faces as usize;
    let mut out = Vec::with_capacity(header + font.len());
    out.extend_from_slice(b"ttcf");
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&faces.to_be_bytes());
    for _ in 0..faces {
        out.extend_from_slice(&(header as u32).to_be_bytes());
    }
    let mut face = font.to_vec();
    let tables = u16::from_be_bytes([face[4], face[5]]) as usize;
    for i in 0..tables {
        let at = 12 + i * 16 + 8;
        let offset = u32::from_be_bytes(face[at..at + 4].try_into().unwrap()) + header as u32;
        face[at..at + 4].copy_from_slice(&offset.to_be_bytes());
    }
    out.extend_from_slice(&face);
    out
}

#[test]
fn a_font_is_found_by_the_names_inside_it() {
    let dir = FontsDir::new("names");
    // Names that say nothing of the font, and an extension in capitals.
    dir.put("a1.TTF", REGULAR);
    dir.put("deeper/b2.ttf", SEMIBOLD);
    dir.put("c3.ttf", MONO);
    let index = dir.index();
    assert!(index.has("inter"), "{:?}", index.names());
    assert!(index.has("jetbrains mono"));
    assert!(!index.has("a1") && !index.has("arial"));
    let regular = index.face("inter", false).unwrap();
    assert_eq!((regular.weight, regular.italic, regular.index), (400, false, 0));
    assert!(regular.path.ends_with("a1.TTF"));
    // The semibold face is Inter's too (by its typographic family name), and draws its bold text.
    let bold = index.face("inter", true).unwrap();
    assert_eq!(bold.weight, 600);
    assert!(bold.path.ends_with("b2.ttf"));
    // A family with no face of 600 or more has none for bold text.
    assert!(index.face("jetbrains mono", true).is_none());
    assert!(index.face("jetbrains mono", false).is_some());
    assert!(index.face("nosuch", false).is_none());
}

/// Where the table `tag` of a face of its own starts.
fn table(font: &[u8], tag: &[u8; 4]) -> usize {
    let tables = u16::from_be_bytes([font[4], font[5]]) as usize;
    (0..tables)
        .map(|i| 12 + i * 16)
        .find(|at| &font[*at..*at + 4] == tag)
        .map(|at| u32::from_be_bytes(font[at + 8..at + 12].try_into().unwrap()) as usize)
        .unwrap()
}

#[test]
fn a_condensed_face_of_the_family_is_not_taken_for_its_regular_one() {
    let dir = FontsDir::new("condensed");
    let mut condensed = REGULAR.to_vec();
    let os2 = table(&condensed, b"OS/2");
    // `usWidthClass` 3: condensed. Its file comes first.
    condensed[os2 + 6..os2 + 8].copy_from_slice(&3u16.to_be_bytes());
    dir.put("a-condensed.ttf", &condensed);
    dir.put("b-regular.ttf", REGULAR);
    let index = dir.index();
    let face = index.face("inter", false).unwrap();
    assert!(face.path.ends_with("b-regular.ttf"), "{face:?}");
    assert_eq!(face.width, 5);
    // Alone, the condensed face is the family's.
    let only = FontsDir::new("condensed-only");
    only.put("a-condensed.ttf", &condensed);
    assert_eq!(only.index().face("inter", false).map(|f| f.width), Some(3));
}

#[test]
fn what_is_not_a_font_is_left_out() {
    let dir = FontsDir::new("junk");
    dir.put("words.ttf", b"This is not a font, whatever it is called.");
    dir.put("empty.otf", b"");
    // Cut off inside its directory, and inside its name table.
    dir.put("cut.ttf", &REGULAR[..40]);
    let name_at = {
        let tables = u16::from_be_bytes([REGULAR[4], REGULAR[5]]) as usize;
        (0..tables)
            .map(|i| 12 + i * 16)
            .find(|at| &REGULAR[*at..*at + 4] == b"name")
            .map(|at| u32::from_be_bytes(REGULAR[at + 8..at + 12].try_into().unwrap()) as usize)
            .unwrap()
    };
    dir.put("cut-name.ttf", &REGULAR[..name_at + 10]);
    // A font, but not by its extension: only font files are opened.
    dir.put("inter.txt", REGULAR);
    // A collection that says it has a billion faces.
    let mut huge = collection(REGULAR, 1);
    huge[8..12].copy_from_slice(&1_000_000_000u32.to_be_bytes());
    dir.put("huge.ttc", &huge);
    let index = dir.index();
    // Only the collection's one real face is in it.
    let faces: Vec<_> = index.families.iter().flat_map(|f| f.faces.iter()).collect();
    assert!(faces.iter().all(|f| f.path.ends_with("huge.ttc") && f.index == 0), "{:?}", index.families);
    assert!(index.has("inter"));
    const { assert!(MAX_COLLECTION <= 64) };
}

#[test]
fn each_face_of_a_collection_is_found_with_its_place_in_it() {
    let dir = FontsDir::new("collection");
    let bytes = collection(MONO, 2);
    dir.put("pair.ttc", &bytes);
    let index = dir.index();
    // Both faces are the same face: the first is kept.
    let face = index.face("jetbrains mono", false).unwrap();
    assert_eq!(face.index, 0);
    // egui is given the face asked for, and only one that is there.
    assert!(crate::theme::is_face(&bytes, 0));
    assert!(crate::theme::is_face(&bytes, 1));
    assert!(!crate::theme::is_face(&bytes, 2));
    assert!(!crate::theme::is_face(MONO, 1));
    assert!(crate::theme::read_face(&dir.0.join("pair.ttc"), u64::MAX, 1).is_some());
    assert!(crate::theme::read_face(&dir.0.join("pair.ttc"), u64::MAX, 5).is_none());
}

/// `font` with its OS/2 `sFamilyClass` major class set to `class`: 1 is Roman, a serif family.
pub(crate) fn as_class(font: &[u8], class: u8) -> Vec<u8> {
    let mut out = font.to_vec();
    let at = table(&out, b"OS/2");
    out[at + 30] = class;
    out[at + 31] = 0;
    out
}

/// `font` with `post`'s `isFixedPitch` set: every character as wide as the next.
fn as_fixed(font: &[u8]) -> Vec<u8> {
    let mut out = font.to_vec();
    let at = table(&out, b"post");
    out[at + 20..at + 24].copy_from_slice(&1u32.to_be_bytes());
    out
}

#[test]
fn a_family_is_listed_once_by_the_name_it_gives_itself() {
    // #1493: what a font menu lists. Inter's semibold file is Inter's bold face, not a second
    // family; every name a file answers to still finds it.
    let dir = FontsDir::new("listed");
    dir.put("inter.ttf", REGULAR);
    dir.put("semibold.ttf", SEMIBOLD);
    dir.put("mono.ttf", MONO);
    let index = dir.index();
    assert_eq!(index.names(), vec!["Inter", "JetBrains Mono"]);
    assert!(index.has("inter") && index.has("inter semibold") && index.has("jetbrains mono"));
}

#[test]
fn what_kind_a_family_is_is_read_from_its_own_file() {
    // What a menu writes after a family's name, so a recipient without it gets a font of the same
    // kind: monospace when its characters are as wide as each other, serif when its own class says
    // so, and nothing else known when it says neither.
    let fixed = FontsDir::new("kind-fixed");
    fixed.put("f.ttf", &as_fixed(REGULAR));
    let index = fixed.index();
    assert!(index.fixed("inter") && !index.serif("inter"));
    // The IBM classes the field is filled with: oldstyle (1), transitional (2), modern (3),
    // Clarendon (4), slab (5) and freeform (7) serifs are serif families.
    for class in [1u8, 2, 3, 4, 5, 7] {
        let serif = FontsDir::new(&format!("kind-serif-{class}"));
        serif.put("s.ttf", &as_class(REGULAR, class));
        let index = serif.index();
        assert!(index.serif("inter") && !index.fixed("inter") && !index.script("inter"), "class {class}");
    }
    // 8 is a sans-serif, not a script: a family of that class must not be written out with
    // `cursive` after it.
    let sans = FontsDir::new("kind-sans");
    sans.put("s.ttf", &as_class(REGULAR, 8));
    let index = sans.index();
    assert!(!index.serif("inter") && !index.script("inter") && !index.fixed("inter"), "class 8 is a sans-serif");
    // 10 is written rather than set: neither serif nor sans-serif describes it.
    let script = FontsDir::new("kind-script");
    script.put("s.ttf", &as_class(REGULAR, 10));
    let index = script.index();
    assert!(index.script("inter") && !index.serif("inter"), "class 10 is a script");
    let plain = FontsDir::new("kind-plain");
    plain.put("p.ttf", REGULAR);
    let index = plain.index();
    assert!(!index.fixed("inter") && !index.serif("inter"));
    // JetBrains Mono says so itself, without being doctored.
    let mono = FontsDir::new("kind-mono");
    mono.put("m.ttf", MONO);
    assert!(mono.index().fixed("jetbrains mono"));
}

#[test]
fn the_names_of_a_name_table_are_read_in_lower_case_once_each() {
    let tables = u16::from_be_bytes([SEMIBOLD[4], SEMIBOLD[5]]) as usize;
    let (at, len) = (0..tables)
        .map(|i| 12 + i * 16)
        .find(|at| &SEMIBOLD[*at..*at + 4] == b"name")
        .map(|at| {
            let n = |o: usize| u32::from_be_bytes(SEMIBOLD[at + o..at + o + 4].try_into().unwrap()) as usize;
            (n(8), n(12))
        })
        .unwrap();
    let (display, names) = family_names(&SEMIBOLD[at..at + len]).unwrap();
    assert_eq!(display, "Inter");
    assert!(names.contains(&"inter".to_string()), "{names:?}");
    let mut unique = names.clone();
    unique.dedup();
    assert_eq!(unique.len(), names.len());
    assert!(names.iter().all(|n| *n == n.to_lowercase()));
    assert!(family_names(b"junk").is_none());
    assert!(family_names(&[]).is_none());
}

/// A `name` table with `aliases` spellings in another language, and `shown` as the English
/// typographic family name after them all.
fn name_table(aliases: u16, shown: &str) -> Vec<u8> {
    let utf16 = |s: &str| s.encode_utf16().flat_map(u16::to_be_bytes).collect::<Vec<u8>>();
    let mut records: Vec<(u16, u16, u16, Vec<u8>)> = Vec::new();
    for i in 0..aliases {
        records.push((3, 1, 0x0407, utf16(&format!("Alias {i}"))));
    }
    records.push((3, 1, 0x0409, utf16(shown)));
    let mut head: Vec<u8> = Vec::new();
    head.extend_from_slice(&0u16.to_be_bytes());
    head.extend_from_slice(&(records.len() as u16).to_be_bytes());
    head.extend_from_slice(&((6 + records.len() * 12) as u16).to_be_bytes());
    let mut storage: Vec<u8> = Vec::new();
    for (platform, encoding, language, text) in &records {
        head.extend_from_slice(&platform.to_be_bytes());
        head.extend_from_slice(&encoding.to_be_bytes());
        head.extend_from_slice(&language.to_be_bytes());
        // 16 is the typographic family name; the rest are plain family names.
        let id: u16 = if language == &0x0409 { 16 } else { 1 };
        head.extend_from_slice(&id.to_be_bytes());
        head.extend_from_slice(&(text.len() as u16).to_be_bytes());
        head.extend_from_slice(&(storage.len() as u16).to_be_bytes());
        storage.extend_from_slice(text);
    }
    head.extend_from_slice(&storage);
    head
}

/// `font` with its `name` table replaced by one that says `shown` and nothing else: a test's way
/// of giving a family a name of its own. Every table after it moves to where the new one leaves
/// room, the way [`collection`] moves the whole file.
pub(crate) fn renamed(font: &[u8], shown: &str) -> Vec<u8> {
    let num = u16::from_be_bytes([font.get(4).copied().unwrap_or(0), font.get(5).copied().unwrap_or(0)]) as usize;
    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    for i in 0..num {
        let at = 12 + i * 16;
        let Some(row) = font.get(at..at + 16) else { break };
        let tag = [row[0], row[1], row[2], row[3]];
        let off = u32::from_be_bytes([row[8], row[9], row[10], row[11]]) as usize;
        let len = u32::from_be_bytes([row[12], row[13], row[14], row[15]]) as usize;
        let body = font.get(off..off.saturating_add(len)).unwrap_or_default().to_vec();
        tables.push((tag, body));
    }
    let fresh = name_table(0, shown);
    let head = 12 + tables.len() * 16;
    let mut out = font.get(..12).unwrap_or_default().to_vec();
    out.resize(head, 0);
    out[4..6].copy_from_slice(&(tables.len() as u16).to_be_bytes());
    let mut at = head;
    for (i, (tag, body)) in tables.iter().enumerate() {
        let body: &[u8] = if tag == b"name" { &fresh } else { body };
        let pad = (4 - body.len() % 4) % 4;
        let row = 12 + i * 16;
        out[row..row + 4].copy_from_slice(tag);
        out[row + 8..row + 12].copy_from_slice(&(at as u32).to_be_bytes());
        out[row + 12..row + 16].copy_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(body);
        out.resize(at + body.len() + pad, 0);
        at += body.len() + pad;
    }
    out
}

#[test]
fn a_font_can_be_given_a_family_name_of_its_own() {
    // The helper the cousin tests lean on: the family is found by the name written into it, and
    // not by the one the file came with.
    let dir = FontsDir::new("renamed");
    dir.put("c.ttf", &renamed(REGULAR, "Calibri"));
    let index = dir.index();
    assert!(index.has("calibri"), "{:?}", index.names());
    assert!(!index.has("inter"), "the name it came with is gone: {:?}", index.names());
}

#[test]
fn the_name_a_family_is_shown_by_is_a_name_it_answers_to() {
    // Thirty-two spellings come before the English one in the file. The name a menu shows must
    // still be one the family is found by, or choosing it in the menu finds nothing (#1493).
    let (display, keys) = family_names(&name_table(32, "Real Family")).unwrap();
    assert_eq!(display, "Real Family");
    assert!(keys.len() <= 32, "{keys:?}");
    assert!(keys.contains(&"real family".to_string()), "the shown name is a key too: {keys:?}");
    // With room to spare, every spelling is a key and nothing is dropped.
    let (display, keys) = family_names(&name_table(3, "Real Family")).unwrap();
    assert_eq!(display, "Real Family");
    assert!(keys.contains(&"alias 0".to_string()) && keys.contains(&"real family".to_string()), "{keys:?}");
}

/// This computer's own fonts are read without trouble, and what the system's folder has is found.
#[test]
fn the_fonts_of_this_system_are_found() {
    let started = std::time::Instant::now();
    let index = FontIndex::system();
    eprintln!("{} families in {:?}", index.len(), started.elapsed());
    #[cfg(windows)]
    {
        let arial = std::env::var_os("WINDIR").map(|w| PathBuf::from(w).join("Fonts").join("arial.ttf"));
        if arial.is_some_and(|a| a.is_file()) {
            assert!(index.has("arial"));
            assert!(index.face("arial", true).is_some_and(|f| f.weight >= 600));
        }
    }
}

#[test]
fn a_missing_folder_has_no_fonts() {
    let index = FontIndex::of_dirs(&[PathBuf::from("no-such-folder-anywhere")]);
    assert!(index.is_empty());
}
