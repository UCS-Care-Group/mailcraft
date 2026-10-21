// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Removing everything MailCraft keeps on this computer (#177): the host's part, in folders of
//! their own in the system's temporary folder (never the real settings).

use super::*;

/// A scratch folder of this test's own in the system's temporary folder (never the real
/// settings): `<tmp>/mailcraft-wipe-<name>-<pid>/`.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("mailcraft-wipe-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
    fn join(&self, p: &str) -> PathBuf {
        self.0.join(p)
    }
    fn file(&self, p: &str, text: &str) -> PathBuf {
        let path = self.join(p);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn roots(scratch: &Scratch) -> Roots {
    let settings = scratch.join("config/mailcraft");
    let attachments = scratch.join("cache/MailCraft/Attachments");
    Roots {
        list: vec![
            Root { path: settings.clone(), name: "mailcraft", itself: true },
            Root { path: attachments.clone(), name: "Attachments", itself: true },
        ],
        parents: vec![Root { path: scratch.join("cache/MailCraft"), name: "MailCraft", itself: true }],
        settings: Some(settings),
    }
}

fn fill(scratch: &Scratch) {
    scratch.file("config/mailcraft/mailbox.mailcraft", "headers");
    scratch.file("config/mailcraft/mailbox.mailcraft.blobs/3f/3f2a", "body");
    scratch.file("config/mailcraft/mailbox.mailcraft.words", "w");
    scratch.file("config/mailcraft/ui.json", "{}");
    scratch.file("config/mailcraft/logs/mailcraft.log", "log");
    scratch.file("config/mailcraft/pictures/ab/abcd", "png");
    scratch.file("cache/MailCraft/Attachments/123-1-0/1/report.pdf", "pdf");
}

#[test]
fn everything_in_the_known_folders_goes_and_nothing_outside_them() {
    let scratch = Scratch::new("all");
    fill(&scratch);
    // Neighbours that are not MailCraft's.
    let other_config = scratch.file("config/other-app/settings", "keep");
    let other_cache = scratch.file("cache/other-app/data", "keep");
    let mine = scratch.file("Documents/export.mailcraft", "the owner's own file");

    let report = erase(&roots(&scratch), None);

    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(report.left.is_empty());
    assert!(!scratch.join("config/mailcraft").exists());
    assert!(!scratch.join("cache/MailCraft").exists());
    assert!(other_config.exists() && other_cache.exists() && mine.exists());
    assert!(report.removed >= 7);
}

#[test]
fn a_second_run_finds_nothing_and_reports_nothing() {
    let scratch = Scratch::new("again");
    fill(&scratch);
    let roots = roots(&scratch);
    erase(&roots, None);
    let report = erase(&roots, None);
    assert_eq!(report, WipeReport::default());
}

#[test]
fn a_parent_that_holds_something_else_stays() {
    let scratch = Scratch::new("parent");
    fill(&scratch);
    let other = scratch.file("cache/MailCraft/Other/keep", "x");
    let report = erase(&roots(&scratch), None);
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(other.exists());
    assert!(!scratch.join("cache/MailCraft/Attachments").exists());
}

#[cfg(unix)]
#[test]
fn a_link_pointing_outside_is_removed_but_never_followed() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new("link");
    fill(&scratch);
    let outside = scratch.file("elsewhere/precious/file.txt", "keep me");
    // A link to a folder and a link to a file, inside the settings folder.
    symlink(scratch.join("elsewhere/precious"), scratch.join("config/mailcraft/logs/dir-link")).unwrap();
    symlink(&outside, scratch.join("config/mailcraft/file-link")).unwrap();
    // And a dangling one.
    symlink(scratch.join("elsewhere/nothing"), scratch.join("config/mailcraft/dangling")).unwrap();

    let report = erase(&roots(&scratch), None);

    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(outside.exists(), "what a link led to was removed");
    assert_eq!(fs::read_to_string(&outside).unwrap(), "keep me");
    assert!(!scratch.join("config/mailcraft").exists());
}

#[cfg(unix)]
#[test]
fn a_root_that_is_itself_a_link_is_left_entirely_alone() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new("rootlink");
    let target = scratch.file("dotfiles/mailcraft/ui.json", "{}");
    fs::create_dir_all(scratch.join("config")).unwrap();
    symlink(scratch.join("dotfiles/mailcraft"), scratch.join("config/mailcraft")).unwrap();

    let report = erase(&roots(&scratch), None);

    assert!(target.exists());
    assert!(report.failed.iter().any(|(_, why)| *why == Why::IsLink), "{:?}", report.failed);
}

#[test]
fn a_root_that_is_not_named_as_ours_is_refused() {
    let scratch = Scratch::new("name");
    let precious = scratch.file("home/Documents/letter.txt", "dear…");
    let wrong = Root { path: scratch.join("home"), name: "mailcraft", itself: true };
    let relative = Root { path: PathBuf::from("mailcraft"), name: "mailcraft", itself: true };
    let up = Root { path: scratch.join("home/../home/mailcraft"), name: "mailcraft", itself: true };
    let roots = Roots { list: vec![wrong, relative, up], ..Default::default() };

    let report = erase(&roots, None);

    assert!(precious.exists());
    assert!(report.failed.len() >= 2, "{:?}", report.failed);
    assert_eq!(report.removed, 0);
}

#[test]
fn a_mailbox_kept_elsewhere_is_left_and_said() {
    let scratch = Scratch::new("elsewhere");
    fill(&scratch);
    let theirs = scratch.file("usb/mail.mailcraft", "theirs");
    let inside = scratch.join("config/mailcraft/mailbox.mailcraft");

    let report = erase(&roots(&scratch), Some(&theirs));
    assert!(theirs.exists());
    assert_eq!(report.left.len(), 1);

    // The one in the settings folder is the program's own: no remark.
    let scratch2 = Scratch::new("inside");
    fill(&scratch2);
    let report = erase(&roots(&scratch2), Some(&scratch2.join("config/mailcraft/mailbox.mailcraft")));
    assert!(report.left.is_empty());
    drop(inside);
}

#[cfg(unix)]
#[test]
fn what_cant_be_removed_is_reported_and_the_rest_still_goes() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("denied");
    fill(&scratch);
    // A folder that can't be written to: its file can't be removed.
    scratch.file("config/mailcraft/locked/stuck", "x");
    let locked = scratch.join("config/mailcraft/locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();
    // (Running as root, nothing is denied: then there is nothing to check.)
    let denied = fs::remove_file(locked.join("stuck")).is_err();

    let report = erase(&roots(&scratch), None);

    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    if denied {
        assert!(!report.failed.is_empty());
    }
    assert!(!scratch.join("config/mailcraft/mailbox.mailcraft").exists());
    assert!(!scratch.join("config/mailcraft/ui.json").exists());
    assert!(!scratch.join("cache/MailCraft").exists());
}

#[test]
fn the_real_folders_are_the_ones_the_program_uses() {
    let roots = Roots::of_this_program();
    if let Some(settings) = mailcraft_control_key::settings_dir() {
        assert!(roots.list.iter().any(|r| r.path == settings));
        assert_eq!(roots.settings.as_deref(), Some(settings.as_path()));
    }
    if let Some(base) = crate::opened_files::base() {
        assert!(roots.list.iter().any(|r| r.path == base));
    }
    // Whatever they are, each passes the checks it will be held to (or does not exist yet).
    for root in &roots.list {
        let named = root.path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.eq_ignore_ascii_case(root.name));
        assert!(named, "{root:?}");
    }
}

// ---- the review of #294 ----

#[test]
fn the_mailbox_file_goes_before_its_content_and_word_index() {
    assert_eq!(tier("mailbox.mailcraft", false), 0);
    assert_eq!(tier("mailbox.mailcraft.format2", false), 0);
    assert_eq!(tier("mailbox.mailcraft.blobs", true), 1);
    assert_eq!(tier("mailbox.mailcraft.words", false), 1);
    assert_eq!(tier("ui.json", false), 2);
    assert_eq!(tier("logs", true), 2);
    // Sorted, as `clear` does: file, then content, then the rest.
    let mut names = [(tier("ui.json", false), "ui.json"), (tier("mailbox.mailcraft.blobs", true), "b"), (tier("mailbox.mailcraft", false), "m")];
    names.sort();
    assert_eq!(names.iter().map(|n| n.1).collect::<Vec<_>>(), ["m", "b", "ui.json"]);
}

#[cfg(unix)]
#[test]
fn a_mailbox_file_that_cannot_go_keeps_its_content_so_nothing_dangles() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("dangling");
    fill(&scratch);
    let settings = scratch.join("config/mailcraft");
    // The folder can't be changed: no file in it can be removed (as a locked file would be).
    fs::set_permissions(&settings, fs::Permissions::from_mode(0o500)).unwrap();
    let denied = fs::remove_file(settings.join("mailbox.mailcraft")).is_err();

    let report = erase(&roots(&scratch), None);

    fs::set_permissions(&settings, fs::Permissions::from_mode(0o700)).unwrap();
    if denied {
        assert!(settings.join("mailbox.mailcraft").exists());
        // Its content is still there: the mailbox that stays is whole.
        assert!(settings.join("mailbox.mailcraft.blobs/3f/3f2a").exists(), "content went while its mailbox stayed");
        assert!(report.failed.iter().any(|(_, why)| *why == Why::MailboxStays), "{:?}", report.failed);
    }
}

#[test]
fn the_content_and_word_index_of_a_mailbox_kept_elsewhere_are_left_and_said() {
    let scratch = Scratch::new("sidecars");
    fill(&scratch);
    let theirs = scratch.file("usb/mail.mailcraft", "theirs");
    scratch.file("usb/mail.mailcraft.blobs/aa/aabb", "body");
    scratch.file("usb/mail.mailcraft.words", "w");

    let report = erase(&roots(&scratch), Some(&theirs));

    assert!(scratch.join("usb/mail.mailcraft.blobs/aa/aabb").exists() && scratch.join("usb/mail.mailcraft.words").exists());
    let named: Vec<&str> = report.left.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(named.len(), 3, "{named:?}");
    assert!(named.iter().any(|p| p.ends_with("mail.mailcraft.blobs")) && named.iter().any(|p| p.ends_with("mail.mailcraft.words")));
}

#[test]
fn inside_the_settings_folder_is_decided_by_where_things_really_are() {
    let scratch = Scratch::new("inside-real");
    let file = scratch.file("config/mailcraft/mailbox.mailcraft", "x");
    let settings = scratch.join("config/mailcraft");
    // By name, with a `..` in the way, it still is.
    let roundabout = scratch.join("config/mailcraft/../mailcraft/mailbox.mailcraft");
    assert!(inside(&file, Some(&settings)));
    assert!(inside(&roundabout, Some(&settings)));
    assert!(!inside(&scratch.file("elsewhere/mail.mailcraft", "x"), Some(&settings)));
    assert!(!inside(&file, None));
    #[cfg(unix)]
    {
        // A settings folder reached through a link (the owner's dotfiles) is the same folder.
        std::os::unix::fs::symlink(&settings, scratch.join("link")).unwrap();
        assert!(inside(&scratch.join("link/mailbox.mailcraft"), Some(&settings)));
        assert!(inside(&file, Some(&scratch.join("link"))));
    }
}

#[test]
fn a_removal_stops_the_pictures_from_writing_and_marks_the_data_as_removed() {
    let scratch = Scratch::new("pictures");
    fill(&scratch);
    let pictures = crate::picture_cache::Cache::at(scratch.join("config/mailcraft/pictures"));
    let wipe = Wipe { roots: roots(&scratch), pictures: Some(pictures.clone()) };
    assert!(!begun() && may_install_at_exit(false));

    wipe.erase(None);

    assert!(begun());
    assert!(!may_install_at_exit(false), "a copy that updates itself would install after the wipe");
    assert!(!may_install_at_exit(true));
    // A download still in flight is not kept, and nothing makes the folder again.
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&[0u8; 64]);
    assert!(!pictures.put("https://example.org/a.png", &png));
    pictures.flush();
    assert!(!scratch.join("config/mailcraft").exists());
}
