// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Removing everything MailCraft keeps on this computer (#177): File › Info › Remove MailCraft's
//! data from this computer. The window stops writing and forgets the system's credential store
//! entries first (`mailcraft_ui_egui::data_removal`); this is the host's part, the folders.
//!
//! ```text
//! <settings>/                   (mailcraft_control_key::settings_dir: %APPDATA%\MailCraft,
//!                                ~/Library/Application Support/MailCraft, ~/.config/mailcraft)
//!     mailbox.mailcraft…        the mailbox, its content folder, its word index
//!     ui.json, logs/, pictures/ the settings, the logs, the downloaded pictures
//!     updates/ …                the update's downloads and the version kept to go back to
//! <cache>/MailCraft/Attachments/    the attachments opened (`opened_files::base`)
//! ```
//!
//! An installed copy and a portable one keep their data in the same places, so one function
//! serves both.
//!
//! # Where it will and won't remove
//!
//! - **Only the folders the program already resolves** ([`Roots`], from the same functions that
//!   find them at run time), never a path from anywhere else: not the command line, not a
//!   message, not a setting. A mailbox file the owner keeps elsewhere (File › Open, `--mailbox`)
//!   is theirs: it is left, and said so.
//! - **Each root is checked before anything in it goes:** absolute, called what it must be
//!   called (`mailcraft`/`MailCraft`, `Attachments`), a real folder and not a link. A root that
//!   fails any of this is reported and left entirely alone.
//! - **Never through a link.** Every entry is looked at with `symlink_metadata`; a link (file or
//!   folder) is itself unlinked and what it points at is never opened, listed or removed. Paths
//!   are only ever made by joining a name `read_dir` gave onto a folder already inside the root,
//!   and each is checked to lie inside the root again right before it goes.
//! - **Bounded:** at most [`MAX_DEPTH`] folders deep and [`MAX_ENTRIES`] entries; past either, the
//!   rest is reported, not followed.
//! - **A failure is a line of the report** and the rest still goes. Nothing panics.
//! - The log file this run writes to is let go of first (a file that is open can't be removed on
//!   Windows), and logging goes to standard error alone afterwards.

use std::fs;
use std::path::{Component, Path, PathBuf};

use mailcraft_ui_egui::data_removal::{DataWipe, Why, WipeReport};

/// Folders deeper than this are not walked (a mailbox's content folder is two deep).
const MAX_DEPTH: usize = 64;
/// Entries one removal looks at before it gives up on the rest (a mailbox of millions of
/// attachments has this many files and more).
const MAX_ENTRIES: usize = 20_000_000;

/// A folder MailCraft keeps data in, and the name it must have.
#[derive(Clone, Debug)]
pub struct Root {
    pub path: PathBuf,
    /// The last component, compared without regard to case (`%APPDATA%\MailCraft`).
    pub name: &'static str,
    /// Remove the folder itself too, not only what is in it.
    pub itself: bool,
}

/// The folders to clear.
#[derive(Clone, Debug, Default)]
pub struct Roots {
    pub list: Vec<Root>,
    /// Folders removed after the roots, only when they are empty by then (`remove_dir` refuses
    /// otherwise): the folder opened attachments sit in is `<cache>/MailCraft/Attachments`.
    pub parents: Vec<Root>,
    /// The settings folder, to tell whether the open mailbox is inside it.
    pub settings: Option<PathBuf>,
}

impl Roots {
    /// The folders this program works in, from the functions it finds them with.
    pub fn of_this_program() -> Roots {
        let mut roots = Roots::default();
        if let Some(settings) = mailcraft_control_key::settings_dir() {
            roots.list.push(Root {
                path: settings.clone(),
                name: if cfg!(any(windows, target_os = "macos")) { "MailCraft" } else { "mailcraft" },
                itself: true,
            });
            roots.settings = Some(settings);
        }
        if let Some(attachments) = crate::opened_files::base() {
            if let Some(cache) = attachments.parent() {
                roots.parents.push(Root { path: cache.to_path_buf(), name: "MailCraft", itself: true });
            }
            roots.list.push(Root { path: attachments, name: "Attachments", itself: true });
        }
        roots
    }
}

/// What the window asks the host for.
pub struct Wipe {
    pub roots: Roots,
}

impl DataWipe for Wipe {
    fn erase(&self, mailbox: Option<&Path>) -> WipeReport {
        // The log file of this run is closed first, so that it can go.
        if let Some(logger) = crate::logging::current() {
            logger.no_file();
        }
        let report = erase(&self.roots, mailbox);
        log::info!("MailCraft's data was removed from this computer ({} entries, {} problems)", report.removed, report.failed.len());
        report
    }
}

/// Remove everything under `roots`; `mailbox` is the file the window had open.
pub fn erase(roots: &Roots, mailbox: Option<&Path>) -> WipeReport {
    let mut report = WipeReport::default();
    let mut budget = MAX_ENTRIES;
    for root in &roots.list {
        clear(root, &mut report, &mut budget);
    }
    // Only when what was in them is gone: `remove_dir` takes an empty folder and nothing else.
    for parent in &roots.parents {
        if checked(parent).is_ok() {
            let _ = fs::remove_dir(&parent.path);
        }
    }
    if let Some(file) = mailbox {
        let inside = roots.settings.as_deref().is_some_and(|settings| file.starts_with(settings));
        if !inside {
            report.left.push((file.display().to_string(), Why::KeptByOwner));
        }
    }
    report
}

/// Whether `root` is a folder this module will remove from: absolute, named as it must be, with
/// no `..` in it, a real folder and no link.
fn checked(root: &Root) -> Result<(), Option<Why>> {
    let path = &root.path;
    let plain = path.is_absolute() && path.components().all(|c| !matches!(c, Component::ParentDir | Component::CurDir));
    let named = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.eq_ignore_ascii_case(root.name));
    if !plain || !named {
        return Err(Some(Why::NotOurs));
    }
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(Some(Why::IsLink)),
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(Some(Why::NotFolder)),
        // Nothing there to remove.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(None),
        Err(e) => Err(Some(Why::Io(e.to_string()))),
    }
}

/// Remove what is in `root` (and the folder, when `root.itself`).
fn clear(root: &Root, report: &mut WipeReport, budget: &mut usize) {
    match checked(root) {
        Ok(()) => {}
        Err(why) => {
            if let Some(why) = why {
                report.failed.push((root.path.display().to_string(), why));
            }
            return;
        }
    }
    let entries = match fs::read_dir(&root.path) {
        Ok(entries) => entries,
        Err(e) => {
            report.failed.push((root.path.display().to_string(), Why::Io(e.to_string())));
            return;
        }
    };
    for entry in entries {
        match entry {
            Ok(entry) => remove_entry(&root.path, &entry.path(), 0, report, budget),
            Err(e) => report.failed.push((root.path.display().to_string(), Why::Io(e.to_string()))),
        }
    }
    if root.itself {
        match fs::remove_dir(&root.path) {
            Ok(()) => report.removed += 1,
            // (Something in it could not go: said above.)
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                if report.failed.is_empty() {
                    report.failed.push((root.path.display().to_string(), Why::Io(e.to_string())));
                }
            }
        }
    }
}

/// Remove `path`, which lies in `root`: a file or a link is removed itself, a folder with what
/// is in it.
fn remove_entry(root: &Path, path: &Path, depth: usize, report: &mut WipeReport, budget: &mut usize) {
    if *budget == 0 {
        if report.failed.last().is_none_or(|(_, why)| *why != Why::TooMany) {
            report.failed.push((root.display().to_string(), Why::TooMany));
        }
        return;
    }
    *budget -= 1;
    // Right before it goes: still inside the root, by its own components.
    if !path.starts_with(root) || path.components().any(|c| matches!(c, Component::ParentDir)) {
        report.failed.push((path.display().to_string(), Why::Outside));
        return;
    }
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        // Gone already (a log rotated away, a run's folder tidied).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            report.failed.push((path.display().to_string(), Why::Io(e.to_string())));
            return;
        }
    };
    if meta.file_type().is_symlink() {
        // The link itself; never what it leads to. (A link to a folder is removed with
        // `remove_dir` on Windows.)
        let gone = fs::remove_file(path).or_else(|_| fs::remove_dir(path));
        done(gone, path, report);
    } else if meta.is_dir() {
        if depth >= MAX_DEPTH {
            report.failed.push((path.display().to_string(), Why::TooDeep));
            return;
        }
        match fs::read_dir(path) {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(entry) => remove_entry(root, &entry.path(), depth + 1, report, budget),
                        Err(e) => report.failed.push((path.display().to_string(), Why::Io(e.to_string()))),
                    }
                }
            }
            Err(e) => report.failed.push((path.display().to_string(), Why::Io(e.to_string()))),
        }
        done(fs::remove_dir(path), path, report);
    } else {
        let mut gone = fs::remove_file(path);
        if gone.is_err() && meta.permissions().readonly() {
            // A copy made read-only (an opened attachment); Windows won't remove those.
            let mut perms = meta.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            if fs::set_permissions(path, perms).is_ok() {
                gone = fs::remove_file(path);
            }
        }
        done(gone, path, report);
    }
}

fn done(result: std::io::Result<()>, path: &Path, report: &mut WipeReport) {
    match result {
        Ok(()) => report.removed += 1,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => report.failed.push((path.display().to_string(), Why::Io(e.to_string()))),
    }
}

#[cfg(test)]
mod tests {
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
}
