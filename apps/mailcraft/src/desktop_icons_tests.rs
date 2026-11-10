// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use super::{ICONS, data_home, icon_path, launcher_path, refresh_in};

/// A data folder of its own for a test (`XDG_DATA_HOME`), empty.
fn data(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mailcraft-desktop-icons-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn put(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

#[test]
fn the_data_folder_is_xdg_data_home_or_under_home() {
    assert_eq!(data_home(Some(OsStr::new("/x/data")), Some(OsStr::new("/home/a"))), Some(PathBuf::from("/x/data")));
    // A relative XDG_DATA_HOME is ignored, as the specification says.
    assert_eq!(data_home(Some(OsStr::new("data")), Some(OsStr::new("/home/a"))), Some(PathBuf::from("/home/a/.local/share")));
    assert_eq!(data_home(Some(OsStr::new("")), Some(OsStr::new("/home/a"))), Some(PathBuf::from("/home/a/.local/share")));
    assert_eq!(data_home(None, Some(OsStr::new("relative"))), None);
    assert_eq!(data_home(None, None), None);
}

#[test]
fn old_icons_beside_the_launcher_are_written_anew() {
    let dir = data("old");
    put(&launcher_path(&dir), b"[Desktop Entry]\n");
    // Two of the sizes were installed, with the old picture; the others weren't.
    put(&icon_path(&dir, "256x256", "png"), b"old picture");
    put(&icon_path(&dir, "48x48", "png"), b"old picture");
    assert_eq!(refresh_in(&dir).unwrap(), 2);
    for (size, ext, bytes) in ICONS {
        let path = icon_path(&dir, size, ext);
        match size {
            "256x256" | "48x48" => assert_eq!(fs::read(&path).unwrap(), bytes),
            // Nothing is added that wasn't there.
            _ => assert!(!path.exists(), "{size}"),
        }
    }
    // As new already: nothing is written, and no file is left beside them.
    assert_eq!(refresh_in(&dir).unwrap(), 0);
    let left: Vec<_> = fs::read_dir(icon_path(&dir, "256x256", "png").parent().unwrap()).unwrap().collect();
    assert_eq!(left.len(), 1);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn without_the_launcher_nothing_is_touched() {
    let dir = data("no-launcher");
    let icon = icon_path(&dir, "256x256", "png");
    put(&icon, b"somebody else's");
    assert_eq!(refresh_in(&dir).unwrap(), 0);
    assert_eq!(fs::read(&icon).unwrap(), b"somebody else's");
    // A data folder that doesn't exist is no fault either.
    assert_eq!(refresh_in(&dir.join("missing")).unwrap(), 0);
    fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn an_icon_that_is_a_link_is_left_alone() {
    let dir = data("link");
    put(&launcher_path(&dir), b"[Desktop Entry]\n");
    let elsewhere = dir.join("elsewhere.png");
    put(&elsewhere, b"a package's");
    let icon = icon_path(&dir, "256x256", "png");
    fs::create_dir_all(icon.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &icon).unwrap();
    assert_eq!(refresh_in(&dir).unwrap(), 0);
    assert_eq!(fs::read(&elsewhere).unwrap(), b"a package's");
    fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn an_icon_in_a_linked_folder_is_left_alone() {
    let dir = data("linked-folder");
    put(&launcher_path(&dir), b"[Desktop Entry]\n");
    // The size's `apps` folder is a link into somebody's own files (GNU Stow, say).
    let theirs = dir.join("dotfiles").join("apps");
    put(&theirs.join("io.github.billsecond.mailcraft.png"), b"their own");
    let icon = icon_path(&dir, "256x256", "png");
    fs::create_dir_all(icon.parent().unwrap().parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&theirs, icon.parent().unwrap()).unwrap();
    assert_eq!(refresh_in(&dir).unwrap(), 0);
    assert_eq!(fs::read(theirs.join("io.github.billsecond.mailcraft.png")).unwrap(), b"their own");
    fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn a_link_left_where_the_new_file_is_written_is_never_followed() {
    let dir = data("planted");
    put(&launcher_path(&dir), b"[Desktop Entry]\n");
    let icon = icon_path(&dir, "256x256", "png");
    put(&icon, b"old picture");
    let elsewhere = dir.join("elsewhere");
    put(&elsewhere, b"not ours");
    let temp = icon.with_file_name("io.github.billsecond.mailcraft.png.new");
    std::os::unix::fs::symlink(&elsewhere, &temp).unwrap();
    assert_eq!(refresh_in(&dir).unwrap(), 1);
    assert_eq!(fs::read(&elsewhere).unwrap(), b"not ours");
    let (_, _, bytes) = ICONS.iter().copied().find(|(size, _, _)| *size == "256x256").unwrap();
    assert_eq!(fs::read(&icon).unwrap(), bytes);
    assert!(fs::symlink_metadata(&temp).is_err(), "nothing left beside it");
    fs::remove_dir_all(&dir).unwrap();
}
