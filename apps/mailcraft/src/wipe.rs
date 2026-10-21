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
//!   are only ever made by joining a name `read_dir` gave onto a folder already inside the root
//!   (a lexical guard repeats that). Links *above* a root (`~/.cache/MailCraft` linked to another
//!   disk) are followed, as the program follows them everywhere; a root that is itself a link is
//!   left alone. A folder swapped for a link in the instant between looking and listing is not
//!   guarded against: that takes someone already able to write the owner's own data.
//! - **In an order that can't leave a mailbox without its content:** the mailbox file goes
//!   first, then its content folder and word index, and when the file can't go they stay.
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

/// Set once a removal has begun: whatever else writes into the data folders (the update's
/// download and its settling, at exit) leaves them alone from then on.
#[cfg(not(test))]
static BEGUN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
// (In the tests, one thread's removal is nobody else's.)
#[cfg(test)]
thread_local! {
    static BEGUN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn begin() {
    #[cfg(not(test))]
    BEGUN.store(true, std::sync::atomic::Ordering::SeqCst);
    #[cfg(test)]
    BEGUN.with(|b| b.set(true));
}

/// Whether MailCraft's data is being, or was, removed from this computer.
pub fn begun() -> bool {
    #[cfg(not(test))]
    return BEGUN.load(std::sync::atomic::Ordering::SeqCst);
    #[cfg(test)]
    return BEGUN.with(|b| b.get());
}

/// Whether a copy that updates itself may install what it downloaded as it exits: not once its
/// data was removed (that writes into the folder again, and could start another version after
/// the owner asked for everything to go).
pub fn may_install_at_exit(data_removed: bool) -> bool {
    !data_removed && !begun()
}

/// What the window asks the host for.
pub struct Wipe {
    pub roots: Roots,
    /// The downloaded pictures kept on disk, told to write nothing before their folder goes.
    pub pictures: Option<std::sync::Arc<crate::picture_cache::Cache>>,
}

impl DataWipe for Wipe {
    fn erase(&self, mailbox: Option<&Path>) -> WipeReport {
        begin();
        if let Some(pictures) = &self.pictures {
            pictures.stop();
        }
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
    if let Some(file) = mailbox
        && !inside(file, roots.settings.as_deref())
    {
        // The file, and what sits beside it (its content folder and word index): all private
        // mail, all left, all said.
        report.left.push((file.display().to_string(), Why::KeptByOwner));
        for beside in [mailcraft_engine::store::blob_dir(file), mailcraft_engine::store::words_file(file)] {
            if fs::symlink_metadata(&beside).is_ok() {
                report.left.push((beside.display().to_string(), Why::KeptByOwner));
            }
        }
    }
    report
}

/// Whether `file` is in the settings folder. By where both really are when they can be found (a
/// relative path, a settings folder reached through a link), else by their names.
fn inside(file: &Path, settings: Option<&Path>) -> bool {
    let Some(settings) = settings else { return false };
    match (fs::canonicalize(file), fs::canonicalize(settings)) {
        (Ok(file), Ok(settings)) => file.starts_with(settings),
        _ => file.starts_with(settings),
    }
}

/// The order a root's own entries go in: the mailbox file and what stands in for it first, then
/// its content and word index, then the rest. A removal that stops half way then leaves content
/// without a mailbox (harmless, swept) and never a mailbox without its content.
fn tier(name: &str, is_dir: bool) -> u8 {
    let mailbox = name.starts_with("mailbox.");
    match (mailbox, is_dir || name.ends_with(".words")) {
        (true, false) => 0,
        (true, true) => 1,
        _ => 2,
    }
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
    let mut found: Vec<(u8, PathBuf)> = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => {
                let is_dir = fs::symlink_metadata(entry.path()).is_ok_and(|m| m.is_dir());
                let name = entry.file_name();
                found.push((tier(&name.to_string_lossy(), is_dir), entry.path()));
            }
            Err(e) => report.failed.push((root.path.display().to_string(), Why::Io(e.to_string()))),
        }
    }
    found.sort();
    // A mailbox file that couldn't go keeps its content and word index with it.
    let mut mailbox_stays = false;
    for (tier, path) in found {
        if tier == 1 && mailbox_stays {
            report.failed.push((path.display().to_string(), Why::MailboxStays));
            continue;
        }
        let before = report.failed.len();
        remove_entry(&root.path, &path, 0, report, budget);
        if tier == 0 && report.failed.len() > before {
            mailbox_stays = true;
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
    // Paths are only ever made by joining a name `read_dir` gave onto a folder inside the root, so
    // this is a guard against a mistake here, not against anyone: it is lexical.
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
    // (Windows: a junction counts as a link here, and is unlinked. Other reparse points, such as
    // a cloud provider's placeholders, count as what they look like and are walked.)
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
        #[allow(unused_mut)]
        let mut gone = fs::remove_file(path);
        // A copy made read-only (an opened attachment); Windows won't remove those. (Only
        // there: on Unix a file's own mode doesn't stop its removal.)
        #[cfg(windows)]
        if gone.is_err() && meta.permissions().readonly() {
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
#[path = "wipe_tests.rs"]
mod tests;
