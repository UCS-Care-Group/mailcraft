// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use std::path::Path;

#[test]
fn key_file_is_named_by_port() {
    let dir = Path::new("/settings/mailcraft");
    assert_eq!(key_path(dir, 7981), dir.join("control-key.local-7981"));
    assert_eq!(key_path(dir, 7982), dir.join("control-key.local-7982"));
}

#[test]
fn flatpak_instance_does_not_change_the_name() {
    // The app and a bridge run as different Flatpak instances (different ids) and must agree.
    // Nothing is read from the environment or /.flatpak-info, so the name is a
    // function of the folder and the port alone.
    let dir = std::env::temp_dir().join("mc-1199-keys");
    let app = key_path(&dir, 7991);
    let bridge = key_path(&dir, 7991);
    assert_eq!(app, bridge);
    assert_eq!(app.file_name().and_then(|n| n.to_str()), Some("control-key.local-7991"));
}

#[test]
fn bridge_finds_the_key_the_app_wrote() {
    let dir = std::env::temp_dir().join(format!("mc-1199-find-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(key_path(&dir, 7991), "abc").unwrap();
    std::fs::write(key_path(&dir, 7992), "other").unwrap();
    assert_eq!(std::fs::read_to_string(key_path(&dir, 7991)).unwrap(), "abc");
    assert_eq!(std::fs::read_to_string(key_path(&dir, 7992)).unwrap(), "other");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn settings_dir_is_the_apps_folder() {
    if let Some(d) = settings_dir() {
        let name = d.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert!(name == "mailcraft" || name == "MailCraft", "{}", d.display());
    }
}
