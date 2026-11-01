// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The desktop's icons of MailCraft in the owner's own data folder, kept as new as the program.
//!
//! A copy put in place by hand (an AppImage linked from `~/.local/bin`, say) often comes with a
//! launcher and icons written into the owner's own data folder (`$XDG_DATA_HOME`, else
//! `~/.local/share`): `applications/io.github.billsecond.mailcraft.desktop` and
//! `icons/hicolor/<size>/apps/io.github.billsecond.mailcraft.png`, where the desktop's panel looks
//! for the icon of the app id. An update replaces the program and not those, so the panel went
//! on showing the old icon. At every start ([`refresh`]), when the launcher is there and an icon
//! there differs from the one this program carries, each icon file that is there is written anew
//! with this program's: never one that isn't there, one that is a link, or anything outside that
//! folder (what a package manager put in the system's folders is its own). The desktop's icon
//! caches are then told, when their tools are installed. Whatever goes wrong is logged and left:
//! it never stops the program.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The application id the desktop knows MailCraft by.
const APP_ID: &str = "io.github.billsecond.mailcraft";

/// The icons this program carries: the folder under `icons/hicolor` each goes in, its extension
/// and its bytes.
pub(crate) const ICONS: [(&str, &str, &[u8]); 9] = [
    ("16x16", "png", include_bytes!("../../../assets/app-icon/hicolor/16x16/apps/io.github.billsecond.mailcraft.png")),
    ("24x24", "png", include_bytes!("../../../assets/app-icon/hicolor/24x24/apps/io.github.billsecond.mailcraft.png")),
    ("32x32", "png", include_bytes!("../../../assets/app-icon/hicolor/32x32/apps/io.github.billsecond.mailcraft.png")),
    ("48x48", "png", include_bytes!("../../../assets/app-icon/hicolor/48x48/apps/io.github.billsecond.mailcraft.png")),
    ("64x64", "png", include_bytes!("../../../assets/app-icon/hicolor/64x64/apps/io.github.billsecond.mailcraft.png")),
    ("128x128", "png", include_bytes!("../../../assets/app-icon/hicolor/128x128/apps/io.github.billsecond.mailcraft.png")),
    ("256x256", "png", include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/io.github.billsecond.mailcraft.png")),
    ("512x512", "png", include_bytes!("../../../assets/app-icon/hicolor/512x512/apps/io.github.billsecond.mailcraft.png")),
    ("scalable", "svg", include_bytes!("../../../assets/app-icon/hicolor/scalable/apps/io.github.billsecond.mailcraft.svg")),
];

/// The owner's own data folder: `$XDG_DATA_HOME` when it is an absolute path (as the XDG Base
/// Directory specification says), else `$HOME/.local/share` when `HOME` is one; else none.
pub(crate) fn data_home(xdg: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    let absolute = |v: Option<&OsStr>| v.filter(|v| !v.is_empty()).map(PathBuf::from).filter(|p| p.is_absolute());
    absolute(xdg).or_else(|| absolute(home).map(|h| h.join(".local").join("share")))
}

/// A regular file (not a link) at `path`.
fn plain_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

/// The icon file of `size` in the data folder `data`.
pub(crate) fn icon_path(data: &Path, size: &str, ext: &str) -> PathBuf {
    data.join("icons").join("hicolor").join(size).join("apps").join(format!("{APP_ID}.{ext}"))
}

/// The launcher in the data folder `data`.
pub(crate) fn launcher_path(data: &Path) -> PathBuf {
    data.join("applications").join(format!("{APP_ID}.desktop"))
}

/// Whether the file at `path` already holds `bytes` (read only when its size is theirs).
fn holds(path: &Path, bytes: &[u8]) -> bool {
    match fs::symlink_metadata(path) {
        Ok(m) if m.len() == u64::try_from(bytes.len()).unwrap_or(u64::MAX) => fs::read(path).is_ok_and(|b| b == bytes),
        _ => false,
    }
}

/// Write `bytes` over the file at `path`, through a file beside it renamed into place.
fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path.file_name().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut temp = name.to_os_string();
    temp.push(".new");
    let temp = path.with_file_name(temp);
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

/// Bring the icons in the data folder `data` up to this program's, when its launcher is there:
/// the number of files written anew (0 when they were all as new already, or there are none).
pub(crate) fn refresh_in(data: &Path) -> io::Result<usize> {
    if !plain_file(&launcher_path(data)) {
        return Ok(0);
    }
    let mut written = 0usize;
    for (size, ext, bytes) in ICONS {
        let path = icon_path(data, size, ext);
        if plain_file(&path) && !holds(&path, bytes) {
            replace(&path, bytes)?;
            written = written.saturating_add(1);
        }
    }
    Ok(written)
}

/// Tell the desktop's icon caches that the icons in `data` changed, with whichever of their
/// tools is installed; none being installed is no fault.
fn nudge(data: &Path) {
    let hicolor = data.join("icons").join("hicolor");
    // (A folder that changed is what an icon cache looks at first.)
    if let Ok(dir) = fs::File::open(&hicolor) {
        let _ = dir.set_modified(std::time::SystemTime::now());
    }
    let tools: [(&str, Vec<&OsStr>); 3] = [
        ("gtk-update-icon-cache", vec![OsStr::new("--force"), OsStr::new("--ignore-theme-index"), OsStr::new("--quiet"), hicolor.as_os_str()]),
        ("kbuildsycoca6", vec![OsStr::new("--noincremental")]),
        ("kbuildsycoca5", vec![OsStr::new("--noincremental")]),
    ];
    for (tool, args) in tools {
        // A program with fixed arguments, never a shell; one that isn't installed is skipped.
        let run = std::process::Command::new(tool)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match run {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("{tool} couldn't be started: {}", e.kind()),
        }
    }
}

/// At start, on a thread of its own: the owner's own icons of MailCraft brought up to this
/// program's ([`refresh_in`]). Not in a Flatpak, whose desktop files are its own.
pub(crate) fn refresh() {
    if std::env::var_os("FLATPAK_ID").is_some() {
        return;
    }
    let Some(data) = data_home(std::env::var_os("XDG_DATA_HOME").as_deref(), std::env::var_os("HOME").as_deref()) else {
        return;
    };
    let spawned = std::thread::Builder::new().name("desktop-icons".into()).spawn(move || match refresh_in(&data) {
        Ok(0) => {}
        Ok(n) => {
            log::info!("brought {n} of the desktop's icons of MailCraft up to this version's");
            nudge(&data);
        }
        Err(e) => log::warn!("the desktop's icons of MailCraft couldn't be brought up to date: {}", e.kind()),
    });
    if let Err(e) = spawned {
        log::warn!("the desktop's icons of MailCraft weren't looked at: {}", e.kind());
    }
}

#[cfg(test)]
#[path = "desktop_icons_tests.rs"]
mod desktop_icons_tests;
