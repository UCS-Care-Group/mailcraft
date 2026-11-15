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
    assert!(index.has("inter"), "{:?}", index.families.keys().collect::<Vec<_>>());
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
    let faces: Vec<_> = index.families.values().flatten().collect();
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
    let names = family_names(&SEMIBOLD[at..at + len]);
    assert!(names.contains(&"inter".to_string()), "{names:?}");
    let mut unique = names.clone();
    unique.dedup();
    assert_eq!(unique.len(), names.len());
    assert!(names.iter().all(|n| *n == n.to_lowercase()));
    assert!(family_names(b"junk").is_empty());
    assert!(family_names(&[]).is_empty());
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
