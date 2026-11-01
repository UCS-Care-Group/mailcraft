// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The system's fallback faces behind the interface's fonts (#217, #452).

use std::path::PathBuf;

use egui::FontFamily;

use super::{FALLBACK_FONTS_MAX, SystemFont, font_definitions, font_definitions_with, is_font};

/// A font that is certainly one: the bundled Inter, standing in for a system's face.
const A_FONT: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");

/// Text in the scripts the bundled fonts lack: Chinese and Hebrew.
const OTHER_SCRIPTS: &str = "Inbox 斯洛文尼亚发票 שלום";

/// A folder to stand in for a system's fonts folders, removed when dropped.
struct FontsDir(PathBuf);

impl FontsDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("mailcraft-fallback-fonts-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        FontsDir(dir)
    }
    fn put(&self, file: &str, bytes: &[u8]) {
        let path = self.0.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    fn dirs(&self) -> Vec<PathBuf> {
        vec![self.0.clone()]
    }
}

impl Drop for FontsDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `face` as the one face of a collection (`ttcf`): the collection's header, then the face with
/// its tables' offsets counted from the start of the collection.
fn collection(face: &[u8]) -> Vec<u8> {
    let mut out = b"ttcf".to_vec();
    out.extend(0x0001_0000u32.to_be_bytes());
    out.extend(1u32.to_be_bytes());
    out.extend(16u32.to_be_bytes());
    let mut face = face.to_vec();
    let tables = usize::from(u16::from_be_bytes([face[4], face[5]]));
    for i in 0..tables {
        let at = 12 + i * 16 + 8;
        let offset = u32::from_be_bytes(face[at..at + 4].try_into().unwrap()) + 16;
        face[at..at + 4].copy_from_slice(&offset.to_be_bytes());
    }
    out.extend(face);
    out
}

/// Lay out text of other scripts in every family `fonts` has: egui panics on font data it cannot
/// open.
fn draws(fonts: egui::FontDefinitions) {
    let families: Vec<FontFamily> = fonts.families.keys().cloned().collect();
    let ctx = egui::Context::default();
    ctx.set_fonts(fonts);
    ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
    ctx.run_ui(egui::RawInput::default(), |ui| {
        for family in &families {
            let font = egui::FontId::new(12.0, family.clone());
            let width = ui.painter().layout_no_wrap(OTHER_SCRIPTS.to_string(), font, egui::Color32::WHITE).size().x;
            assert!(width > 20.0);
        }
    })
    .drop_without_applying_deltas();
}

fn fallback_names(fonts: &egui::FontDefinitions) -> Vec<&String> {
    fonts.font_data.keys().filter(|name| name.starts_with("SystemFallback")).collect()
}

/// A fallback face found in a fonts folder, however deep and however its name is cased, goes
/// behind every other font of every family, so that what those draw doesn't change.
#[test]
fn a_fallback_face_goes_behind_every_other_font() {
    let dir = FontsDir::new("behind");
    dir.put("noto/hebrew/Hebrew-Face.TTF", A_FONT);
    let system = SystemFont::none().with_fallbacks_from(&dir.dirs(), &[&["missing.ttf", "hebrew-face.ttf"]]);
    assert_eq!(system.fallback_count(), 1);
    assert!(!system.is_present(), "a fallback face is not the interface's own font");
    let fonts = font_definitions_with(&system);
    assert_eq!(fallback_names(&fonts), ["SystemFallback0"]);
    for (family, faces) in &fonts.families {
        assert_eq!(faces.last().map(String::as_str), Some("SystemFallback0"), "{family:?}");
        // What came before is as it was without it.
        let without = font_definitions_with(&SystemFont::none());
        assert_eq!(&faces[..faces.len() - 1], without.families[family].as_slice(), "{family:?}");
    }
    let proportional = &fonts.families[&FontFamily::Proportional];
    assert_eq!(proportional.first().map(String::as_str), Some("Inter"));
    assert_eq!(proportional.get(proportional.len() - 2).map(String::as_str), Some("Mono"));
    draws(fonts);
}

/// #217: a collection of faces (`ttcf`, as Chinese, Japanese and Korean faces come) is read by
/// its first face, and egui draws with it; a collection that isn't one is left out.
#[test]
fn a_collection_is_read_by_its_first_face() {
    let ttc = collection(A_FONT);
    assert!(is_font(&ttc));
    let no_faces = [&ttc[..8], &0u32.to_be_bytes(), &ttc[12..]].concat();
    assert!(!is_font(&no_faces));
    let past_the_end = [&ttc[..12], &u32::MAX.to_be_bytes(), &ttc[16..]].concat();
    assert!(!is_font(&past_the_end));
    assert!(!is_font(&ttc[..ttc.len() / 2]), "cut off inside its tables");
    assert!(!is_font(&ttc[..14]), "cut off in its header");

    let dir = FontsDir::new("collection");
    dir.put("NotoSansCJK-Regular.ttc", &ttc);
    let system = SystemFont::none().with_fallbacks_from(&dir.dirs(), &[&["NotoSansCJK-Regular.ttc"]]);
    assert_eq!(system.fallback_count(), 1);
    draws(font_definitions_with(&system));
}

/// A face too big, or a file that isn't a font, is passed over for the next face of its script,
/// and with none left the fonts are as they are without a fallback; never a panic.
#[test]
fn a_face_too_big_or_not_a_font_is_passed_over() {
    let dir = FontsDir::new("passed-over");
    dir.put("text.ttf", b"This is not a font, whatever it is called.");
    dir.put("empty.ttf", b"");
    dir.put("big.ttc", &collection(A_FONT));
    dir.put("good.ttf", A_FONT);
    std::fs::create_dir_all(dir.0.join("folder.ttf")).unwrap();
    let cap = A_FONT.len() as u64;
    let group: &[&str] = &["folder.ttf", "text.ttf", "empty.ttf", "big.ttc", "good.ttf"];
    let system = SystemFont::none().with_fallbacks_capped(&dir.dirs(), &[group], cap);
    assert_eq!(system.fallback_count(), 1);
    draws(font_definitions_with(&system));

    let none_good: &[&str] = &["folder.ttf", "text.ttf", "empty.ttf", "big.ttc"];
    let system = SystemFont::none().with_fallbacks_capped(&dir.dirs(), &[none_good], cap);
    assert_eq!(system.fallback_count(), 0);
    let fonts = font_definitions_with(&system);
    assert!(fallback_names(&fonts).is_empty());
    draws(fonts);

    // Folders that aren't there.
    let missing = vec![dir.0.join("no-such-folder"), PathBuf::new()];
    assert_eq!(SystemFont::none().with_fallbacks_from(&missing, &[group]).fallback_count(), 0);
}

/// A face that covers several scripts is read once, no more than so many faces are read, and the
/// walk goes only so deep and follows no link to a folder.
#[test]
fn faces_are_read_once_and_only_so_many() {
    let dir = FontsDir::new("once");
    dir.put("same.ttf", A_FONT);
    dir.put("other.ttf", A_FONT);
    let system = SystemFont::none().with_fallbacks_from(&dir.dirs(), &[&["same.ttf"], &["same.ttf", "other.ttf"], &["other.ttf"]]);
    // The second script is covered by the first's face, and the third has its own.
    assert_eq!(system.fallback_count(), 2);

    let names: Vec<String> = (0..FALLBACK_FONTS_MAX + 4).map(|i| format!("face{i}.ttf")).collect();
    for name in &names {
        dir.put(name, A_FONT);
    }
    let groups: Vec<Vec<&str>> = names.iter().map(|name| vec![name.as_str()]).collect();
    let groups: Vec<&[&str]> = groups.iter().map(Vec::as_slice).collect();
    let system = SystemFont::none().with_fallbacks_from(&dir.dirs(), &groups);
    assert_eq!(system.fallback_count(), FALLBACK_FONTS_MAX);
    draws(font_definitions_with(&system));

    dir.put("1/2/3/4/shallow.ttf", A_FONT);
    dir.put("1/2/3/4/5/deep.ttf", A_FONT);
    assert_eq!(SystemFont::none().with_fallbacks_from(&dir.dirs(), &[&["shallow.ttf"]]).fallback_count(), 1);
    assert_eq!(SystemFont::none().with_fallbacks_from(&dir.dirs(), &[&["deep.ttf"]]).fallback_count(), 0);

    // A link back to the folder itself is not walked round and round.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&dir.0, dir.0.join("loop")).unwrap();
        assert_eq!(SystemFont::none().with_fallbacks_from(&dir.dirs(), &[&["same.ttf"]]).fallback_count(), 1);
    }
}

/// Whether every family of `fonts` has a glyph (not an empty box) for each of `chars`.
fn has_glyphs(fonts: egui::FontDefinitions, chars: &[char]) -> bool {
    let families: Vec<FontFamily> = fonts.families.keys().cloned().collect();
    let ctx = egui::Context::default();
    ctx.set_fonts(fonts);
    ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
    let mut all = true;
    ctx.run_ui(egui::RawInput::default(), |ui| {
        for family in &families {
            let font = egui::FontId::new(12.0, family.clone());
            all &= chars.iter().all(|c| ui.fonts_mut(|f| f.has_glyph(&font, *c)));
        }
    })
    .drop_without_applying_deltas();
    all
}

/// What this system has, whatever that is, goes behind the other fonts and draws text in other
/// scripts: with real glyphs, where it has a face for Chinese or Hebrew.
#[test]
fn the_fallbacks_of_this_system_draw() {
    // The bundled fonts alone draw Chinese and Hebrew as empty boxes, which the check sees.
    assert!(!has_glyphs(font_definitions_with(&SystemFont::none()), &['斯']));
    assert!(!has_glyphs(font_definitions_with(&SystemFont::none()), &['ש']));

    let system = SystemFont::load();
    let fonts = font_definitions();
    assert_eq!(fallback_names(&fonts).len(), system.fallback_count());
    draws(fonts.clone());
    // Windows keeps its faces in one folder, which `font_dirs` doesn't name.
    #[cfg(not(windows))]
    {
        let found = super::find_font_files(&super::font_dirs(), super::FALLBACK_FACES);
        // Only a face that can be read counts: one over the size cap or refused is left out, and
        // the program rightly draws boxes then.
        let has = |names: &[&str]| {
            names.iter().any(|name| found.get(&name.to_lowercase()).is_some_and(|path| super::read_font(path, super::FALLBACK_FONT_CAP).is_some()))
        };
        if super::FALLBACK_FACES.first().is_some_and(|chinese| has(chinese)) {
            assert!(has_glyphs(fonts.clone(), &['斯', '洛', '票']), "Chinese is drawn with {found:?}");
        }
        if has(&["NotoSansHebrew-Regular.ttf", "DejaVuSans.ttf", "ArialHB.ttc", "arial.ttf"]) {
            assert!(has_glyphs(fonts, &['ש', 'ל', 'ו', 'ם']), "Hebrew is drawn with {found:?}");
        }
    }
}

/// A collection that says it holds more faces than the file has room for is refused: egui's font
/// reader finds no first face in it and egui panicked on it at every start (#217, review).
#[test]
fn a_collection_with_too_many_faces_is_refused() {
    let ttc = collection(A_FONT);
    let too_many = [&ttc[..8], &1_000_000u32.to_be_bytes(), &ttc[12..]].concat();
    assert!(!is_font(&too_many));
    // Room for the places of two faces, and only one there.
    let two = [&ttc[..8], &2u32.to_be_bytes(), &ttc[12..16]].concat();
    assert!(!is_font(&two));
    assert!(!is_font(&ttc[..20]), "cut off after its header");
    // A single face whose table directory runs past the end of the file.
    let mut directory_too_long = A_FONT.to_vec();
    directory_too_long[4..6].copy_from_slice(&u16::MAX.to_be_bytes());
    assert!(!is_font(&directory_too_long));
    assert!(!is_font(&A_FONT[..12]));

    // In the system's fonts folder, under the name of the face wanted: left out, never a panic.
    let dir = FontsDir::new("too-many");
    dir.put("NotoSansCJK-Regular.ttc", &too_many);
    dir.put("NotoSansHebrew-Regular.ttf", &directory_too_long);
    let groups: &[&[&str]] = &[&["NotoSansCJK-Regular.ttc"], &["NotoSansHebrew-Regular.ttf"]];
    let system = SystemFont::none().with_fallbacks_from(&dir.dirs(), groups);
    assert_eq!(system.fallback_count(), 0);
    let fonts = font_definitions_with(&system);
    assert!(fallback_names(&fonts).is_empty());
    draws(fonts);
    // Nor as the system's own interface font.
    dir.put("segoeui.ttf", &too_many);
    let system = SystemFont::from_dir(&dir.0);
    assert!(!system.is_present());
    draws(font_definitions_with(&system));
}

/// Each fonts folder is looked through with its own count of entries: a crowded first folder
/// doesn't hide the faces of the next.
#[test]
fn a_crowded_folder_does_not_hide_the_next() {
    let crowded = FontsDir::new("crowded");
    for i in 0..=super::FONT_DIR_ENTRIES {
        std::fs::write(crowded.0.join(format!("{i}.txt")), b"").unwrap();
    }
    let next = FontsDir::new("next");
    next.put("wanted.ttf", A_FONT);
    let dirs = vec![crowded.0.clone(), next.0.clone()];
    assert_eq!(SystemFont::none().with_fallbacks_from(&dirs, &[&["wanted.ttf"]]).fallback_count(), 1);
}
