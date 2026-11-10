// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A settings file that can't be read is set aside without losing an earlier one, and the window
//! is told; one that can't be set aside is left where it is.

use std::path::{Path, PathBuf};

use super::*;

/// A folder of its own for one test, removed when it is done.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("mailcraft-settings-unread-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

/// What is written comes back whole, with nothing left beside it.
#[test]
fn settings_written_come_back_whole() {
    let s = Scratch::new("written");
    let path = s.0.join("ui.json");
    let ui = UiState::default();
    write_prefs(&path, &ui).unwrap();
    let back = read_prefs(&path).0.unwrap();
    assert_eq!(serde_json::to_value(&back).unwrap(), serde_json::to_value(&ui).unwrap());
    assert_eq!(names(&s.0), ["ui.json"]);
}

#[test]
fn a_good_file_is_read_and_a_missing_one_is_nothing_to_say() {
    let s = Scratch::new("good");
    let path = s.file("ui.json", r#"{"theme": "dark", "view": {"sort": "from"}}"#);
    let (ui, unread) = read_prefs(&path);
    assert_eq!(unread, None);
    assert_eq!(ui.unwrap().view.sort, mailcraft_engine::SortBy::From);
    assert_eq!(names(&s.0), ["ui.json"]);
    let (ui, unread) = read_prefs(&s.0.join("nothing.json"));
    assert!(ui.is_none() && unread.is_none());
}

#[test]
fn a_file_that_doesnt_parse_is_set_aside_and_said() {
    let s = Scratch::new("aside");
    let bad = r#"{"theme": "dark", "view": {"sort": "colour"}}"#;
    let path = s.file("ui.json", bad);
    let (ui, unread) = read_prefs(&path);
    assert!(ui.is_none());
    assert_eq!(unread, Some(SettingsUnread::SetAside("ui.json.damaged".into())));
    assert_eq!(names(&s.0), ["ui.json.damaged"]);
    assert_eq!(std::fs::read_to_string(s.0.join("ui.json.damaged")).unwrap(), bad);

    // Another one later keeps the first.
    s.file("ui.json", "not json at all");
    assert_eq!(read_prefs(&path).1, Some(SettingsUnread::SetAside("ui.json.damaged-2".into())));
    assert_eq!(names(&s.0), ["ui.json.damaged", "ui.json.damaged-2"]);
    assert_eq!(std::fs::read_to_string(s.0.join("ui.json.damaged")).unwrap(), bad);
}

/// Where the folder can't be written, the file stays, and nothing claims a copy was kept.
#[cfg(unix)]
#[test]
fn a_file_that_cant_be_set_aside_is_left_in_place() {
    use std::os::unix::fs::PermissionsExt;
    let s = Scratch::new("stuck");
    let path = s.file("ui.json", "{broken");
    std::fs::set_permissions(&s.0, std::fs::Permissions::from_mode(0o555)).unwrap();
    // Root can write anyway; then there is nothing to show.
    let writable = std::fs::write(s.0.join("probe"), "").is_ok();
    let (ui, unread) = read_prefs(&path);
    std::fs::set_permissions(&s.0, std::fs::Permissions::from_mode(0o755)).unwrap();
    if writable {
        return;
    }
    assert!(ui.is_none());
    assert_eq!(unread, Some(SettingsUnread::LeftInPlace));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{broken");
}

/// A dangling link on a name must not be replaced: the next free name is used.
#[cfg(unix)]
#[test]
fn a_dangling_link_on_a_name_is_not_replaced() {
    let s = Scratch::new("dangling");
    std::os::unix::fs::symlink(s.0.join("nowhere"), s.0.join("ui.json.damaged")).unwrap();
    let path = s.file("ui.json", "{broken");
    assert_eq!(read_prefs(&path).1, Some(SettingsUnread::SetAside("ui.json.damaged-2".into())));
    assert!(std::fs::symlink_metadata(s.0.join("ui.json.damaged")).unwrap().file_type().is_symlink());
    assert!(!s.0.join("nowhere").exists());
}

/// Past the cap the oldest copy is replaced, and settings go on being saved.
#[test]
fn past_the_cap_the_oldest_copy_is_replaced() {
    let s = Scratch::new("cap");
    for n in 1..=MAX_DAMAGED_COPIES {
        let name = if n == 1 { "ui.json.damaged".to_string() } else { format!("ui.json.damaged-{n}") };
        s.file(&name, &format!("copy {n}"));
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000 + (MAX_DAMAGED_COPIES - n) as u64);
        std::fs::File::options().write(true).open(s.0.join(&name)).unwrap().set_modified(when).unwrap();
    }
    let path = s.file("ui.json", "{newest");
    let last = format!("ui.json.damaged-{MAX_DAMAGED_COPIES}");
    // The highest number is the oldest here.
    let unread = read_prefs(&path).1;
    assert_eq!(unread, Some(SettingsUnread::SetAside(last.clone())));
    assert_eq!(std::fs::read_to_string(s.0.join(&last)).unwrap(), "{newest");
    assert!(!path.exists());
    assert_eq!(names(&s.0).len(), MAX_DAMAGED_COPIES);
}

/// A name that is a folder is never replaced, and if every name is such, the file stays.
#[test]
fn nothing_is_replaced_that_is_not_a_plain_file() {
    let s = Scratch::new("dirs");
    for n in 1..=MAX_DAMAGED_COPIES {
        let name = if n == 1 { "ui.json.damaged".to_string() } else { format!("ui.json.damaged-{n}") };
        std::fs::create_dir(s.0.join(name)).unwrap();
    }
    let path = s.file("ui.json", "{broken");
    assert!(set_aside(&path).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{broken");
}

/// A settings file that can't be read at all (here a folder) says so, not "couldn't be renamed".
#[test]
fn a_file_that_cant_be_read_is_said_so() {
    let s = Scratch::new("unreadable");
    let path = s.0.join("ui.json");
    std::fs::create_dir(&path).unwrap();
    assert_eq!(read_prefs(&path).1, Some(SettingsUnread::CouldntRead));
    assert!(path.is_dir());
}
