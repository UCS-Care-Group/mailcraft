// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where this system keeps its fonts, and which of them draw the scripts the bundled faces lack
//! (Chinese, Japanese, Korean, Hebrew, Arabic, Thai…): found by the names of their files, for
//! the window (#217, #452) and for printing (#32) alike, wherever a print is made (the window,
//! its control channel, MCP, the command line). Only paths are found here: nothing is read but
//! the folders' entries. The browser has none.

/// How deep into each fonts folder, and through how many of its entries, the fallback faces are
/// looked for: the folders are the system's, but a link may lead anywhere. Each folder has its
/// own count, so that a crowded folder of the owner's doesn't keep the system's from being read.
/// (The same walk finds the fonts a message may be drawn in, in the window.)
#[cfg(not(target_arch = "wasm32"))]
pub const FONT_DIR_DEPTH: usize = 4;
#[cfg(not(target_arch = "wasm32"))]
pub const FONT_DIR_ENTRIES: usize = 20_000;

/// The system faces that draw the scripts the bundled fonts lack (#217, #452), by the names of
/// their files: one group for each script, its faces best first, and the first that is there and
/// readable is used. A face that covers a later group too is read once.
#[cfg(windows)]
pub const FALLBACK_FACES: &[&[&str]] = &[
    // Chinese and Japanese (Microsoft YaHei, Yu Gothic, Meiryo, SimSun), then Korean (Malgun Gothic).
    &["msyh.ttc", "YuGothR.ttc", "meiryo.ttc", "simsun.ttc"],
    &["malgun.ttf", "gulim.ttc"],
    // Segoe UI has Hebrew and Arabic; Thai and the scripts of India come from these.
    &["LeelawUI.ttf", "leelawad.ttf", "tahoma.ttf"],
    &["Nirmala.ttf", "mangal.ttf"],
    // A broad last resort, for when Segoe UI isn't there: Arial has Hebrew and Arabic.
    &["arial.ttf"],
];

/// As on Windows, the faces macOS keeps in its fonts folders (`/System/Library/Fonts` and its
/// `Supplemental`). PingFang is no longer among them on recent systems, so it comes last; Arial
/// Unicode, with Chinese, Japanese, Korean, Hebrew, Arabic, Thai and Devanagari in one face, stands
/// behind each script's own face.
#[cfg(target_os = "macos")]
pub const FALLBACK_FACES: &[&[&str]] = &[
    &["Hiragino Sans GB.ttc", "STHeiti Light.ttc", "STHeiti Medium.ttc", "Arial Unicode.ttf", "PingFang.ttc"],
    &["AppleSDGothicNeo.ttc", "AppleGothic.ttf", "Arial Unicode.ttf"],
    &["ArialHB.ttc", "Arial Unicode.ttf"],
    &["GeezaPro.ttc", "Arial Unicode.ttf"],
    &["Thonburi.ttc", "Arial Unicode.ttf"],
    &["Kohinoor.ttc", "DevanagariMT.ttc", "Arial Unicode.ttf"],
];

/// As on Windows, the faces the free desktops' packages install (Noto, Source Han, WenQuanYi,
/// Nanum, DejaVu), in any of their fonts folders.
#[cfg(all(not(windows), not(target_os = "macos"), not(target_arch = "wasm32")))]
pub const FALLBACK_FACES: &[&[&str]] = &[
    &[
        "NotoSansCJK-Regular.ttc",
        "NotoSansCJKsc-Regular.otf",
        "NotoSansSC-Regular.otf",
        "NotoSansCJKjp-Regular.otf",
        "NotoSansJP-Regular.otf",
        "SourceHanSans-Regular.ttc",
        "SourceHanSansSC-Regular.otf",
        "NotoSansCJK-VF.ttc",
        // Upstream's variable collection is 32,682,580 bytes, close to FALLBACK_FONT_CAP.
        "NotoSansCJK-VF.otf.ttc",
        "wqy-microhei.ttc",
        "wqy-zenhei.ttc",
        "DroidSansFallbackFull.ttf",
        "DroidSansFallback.ttf",
    ],
    &["NotoSansCJK-Regular.ttc", "NotoSansCJKkr-Regular.otf", "NotoSansKR-Regular.otf", "NanumGothic.ttf", "UnDotum.ttf"],
    &["NotoSansHebrew-Regular.ttf"],
    &["NotoSansArabic-Regular.ttf", "NotoNaskhArabic-Regular.ttf"],
    &["NotoSansThai-Regular.ttf"],
    &["NotoSansDevanagari-Regular.ttf"],
    // A broad last resort: Hebrew, Arabic, Armenian, Georgian and more.
    &["DejaVuSans.ttf", "FreeSans.ttf"],
];

/// Where this system keeps its fonts: the owner's own folders first, then the system's.
#[cfg(all(not(windows), not(target_arch = "wasm32")))]
pub fn font_dirs() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
    let mut dirs: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        dirs.extend(home.iter().map(|h| h.join("Library/Fonts")));
        dirs.push("/Library/Fonts".into());
        dirs.push("/System/Library/Fonts".into());
    } else {
        // The XDG base directories, as fontconfig reads them, and its older `~/.fonts`.
        let data_home =
            std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()).map(PathBuf::from).or_else(|| home.as_ref().map(|h| h.join(".local/share")));
        dirs.extend(data_home.map(|d| d.join("fonts")));
        dirs.extend(home.iter().map(|h| h.join(".fonts")));
        // Flatpak and other containers expose host fonts here. Missing paths are harmless.
        #[cfg(target_os = "linux")]
        dirs.extend(["/run/host/user-fonts", "/run/host/local-fonts", "/run/host/fonts"].map(PathBuf::from));
        let data_dirs = std::env::var_os("XDG_DATA_DIRS").filter(|d| !d.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        dirs.extend(std::env::split_paths(&data_dirs).filter(|d| d.is_absolute()).map(|d| d.join("fonts")));
        dirs.push("/usr/local/share/fonts".into());
        dirs.push("/usr/share/fonts".into());
    }
    let mut unique: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        if !unique.contains(&dir) {
            unique.push(dir);
        }
    }
    unique
}

/// The files `faces` names, by their names in lower case, found in `dirs` and the folders in
/// them; the first of a name that is found is kept. The walk goes [`FONT_DIR_DEPTH`] folders deep
/// and through [`FONT_DIR_ENTRIES`] entries of each of `dirs` at most, and follows no link to a
/// folder.
#[cfg(not(target_arch = "wasm32"))]
pub fn find_font_files(dirs: &[std::path::PathBuf], faces: &[&[&str]]) -> std::collections::HashMap<String, std::path::PathBuf> {
    let wanted: std::collections::HashSet<String> = faces.iter().flat_map(|group| group.iter()).map(|name| name.to_lowercase()).collect();
    let mut found = std::collections::HashMap::new();
    for root in dirs {
        find_font_files_in(root, &wanted, &mut found);
    }
    found
}

/// [`find_font_files`] in one of its folders, with that folder's own count of entries.
#[cfg(not(target_arch = "wasm32"))]
fn find_font_files_in(
    root: &std::path::Path,
    wanted: &std::collections::HashSet<String>,
    found: &mut std::collections::HashMap<String, std::path::PathBuf>,
) {
    let mut entries = 0usize;
    let mut pending: Vec<(std::path::PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = pending.pop() {
        let Ok(list) = std::fs::read_dir(&dir) else { continue };
        let mut inner = Vec::new();
        for entry in list.flatten() {
            entries = entries.saturating_add(1);
            if entries > FONT_DIR_ENTRIES {
                return;
            }
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if depth < FONT_DIR_DEPTH {
                    inner.push((entry.path(), depth.saturating_add(1)));
                }
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_lowercase) else { continue };
            // A plain file, where a link leads (opening anything else, a pipe, could wait forever).
            if wanted.contains(&name) && !found.contains_key(&name) && std::fs::metadata(entry.path()).is_ok_and(|m| m.is_file()) {
                found.insert(name, entry.path());
            }
        }
        // The folders inside, in the order of their names.
        inner.sort();
        pending.extend(inner.into_iter().rev());
    }
}

/// The largest fallback face a print takes ([`mailcraft_print::MAX_FACE_BYTES`]).
#[cfg(not(target_arch = "wasm32"))]
const PRINT_FACE_BYTES: u64 = mailcraft_print::MAX_FACE_BYTES as u64;

/// The folders the fallback faces are looked for in: on Windows the system's fonts folder, else
/// [`font_dirs`].
#[cfg(windows)]
pub fn fallback_dirs() -> Vec<std::path::PathBuf> {
    ["WINDIR", "SystemRoot"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(|root| std::path::PathBuf::from(root).join("Fonts"))
        .find(|dir| dir.is_dir())
        .into_iter()
        .collect()
}

#[cfg(all(not(windows), not(target_arch = "wasm32")))]
pub fn fallback_dirs() -> Vec<std::path::PathBuf> {
    font_dirs()
}

/// The files of the fallback faces a print may draw other scripts in: for each group of
/// [`FALLBACK_FACES`], the files of it that are there and not too large to print with, best
/// first (a print takes the first of each group it can read, [`crate::Session::print_fonts`]).
/// Looked for once and kept for the life of the program.
#[cfg(not(target_arch = "wasm32"))]
pub fn fallback_paths() -> Vec<Vec<std::path::PathBuf>> {
    static FOUND: std::sync::OnceLock<Vec<Vec<std::path::PathBuf>>> = std::sync::OnceLock::new();
    FOUND
        .get_or_init(|| {
            let found = find_font_files(&fallback_dirs(), FALLBACK_FACES);
            FALLBACK_FACES
                .iter()
                .map(|group| {
                    group
                        .iter()
                        .filter_map(|name| found.get(&name.to_lowercase()))
                        .filter(|path| std::fs::metadata(path).is_ok_and(|m| m.len() > 0 && m.len() <= PRINT_FACE_BYTES))
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .filter(|group| !group.is_empty())
                .collect()
        })
        .clone()
}

/// None in the browser.
#[cfg(target_arch = "wasm32")]
pub fn fallback_paths() -> Vec<Vec<std::path::PathBuf>> {
    Vec::new()
}
