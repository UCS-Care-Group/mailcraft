// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Attachments the owner opened (#126): written into a folder of MailCraft's own, then opened
//! with the system's program for their kind.
//!
//! ```text
//! <cache>/MailCraft/Attachments/      this user's cache folder (Windows %LOCALAPPDATA%,
//!                                     macOS ~/Library/Caches, elsewhere $XDG_CACHE_HOME or ~/.cache);
//!                                     only this user may read it
//!     <process>-<time>/               one per run of MailCraft, removed when it quits
//!         1/report.pdf                one folder per file opened, so two files of one name
//!         2/report.pdf                never meet; the file read-only, as a copy is
//! ```
//!
//! - **Only what the window allowed:** the window refuses programs, scripts and the like and
//!   asks about macros before this is called (`mailcraft_engine::opening`); the kind is checked
//!   once more here, and the name is made safe once more (no folders, nothing hidden or
//!   option-like, no name Windows keeps for itself, not too long).
//! - **Ours alone:** the folders are made for this user only (0700 on Unix), a link where one of
//!   them should be is never followed, and a file is only ever made new, never written through
//!   something that was there.
//! - **Marked as from the Internet** on Windows (its zone, as a browser's download is) and macOS
//!   (quarantined), so the program that opens it treats it as it treats a download (Office's
//!   Protected View, Gatekeeper); see `mailcraft_engine::mark`.
//! - **Opened without a shell:** the system's own way of opening a file (ShellExecute on Windows,
//!   `open` on macOS, `xdg-open` and the like elsewhere, through the `open` crate), with the
//!   file's whole path as the one argument.
//! - **Tidied:** the run's folder goes when MailCraft quits; what an earlier run left (it was
//!   killed, the file was still open) goes at the next start once it is a day old.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime};

use mailcraft_engine::opening::{self, Verdict};

/// What an earlier run left is removed at the next start once it is this old.
pub const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// How deep the folders go below the one for all runs (run, file's folder, file).
const DEPTH: usize = 3;

/// The folder for all runs: in this user's cache folder.
pub fn base() -> Option<PathBuf> {
    let cache = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
    }?;
    cache.is_absolute().then(|| cache.join("MailCraft").join("Attachments"))
}

/// The files opened in this run: where they go, and how many there have been.
pub struct Opened {
    base: PathBuf,
    run: Mutex<Option<PathBuf>>,
    count: AtomicU32,
}

impl Opened {
    pub fn at(base: PathBuf) -> Opened {
        Opened { base, run: Mutex::new(None), count: AtomicU32::new(0) }
    }

    /// Write `bytes` as an attachment called `name` and open it with the system's program.
    pub fn open(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let path = self.write(name, bytes)?;
        open::that_detached(&path).map_err(|e| {
            log::warn!("couldn't open an attachment with the system's program ({:?})", e.kind());
            "No program could be started to open the attachment: save it with Save As instead.".to_string()
        })
    }

    /// Write the file where it is opened from (the work of [`open`](Self::open) before the
    /// system is asked), and say where.
    pub fn write(&self, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
        if opening::verdict(name) == Verdict::Blocked {
            return Err(opening::blocked_message(name));
        }
        let failed = |e: std::io::Error| {
            log::warn!("couldn't write an attachment to open it ({:?})", e.kind());
            "The attachment couldn't be written to be opened: save it with Save As instead.".to_string()
        };
        let run = self.run_folder().map_err(failed)?;
        let n = self.count.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        let folder = run.join(n.to_string());
        private_dir(&folder).map_err(failed)?;
        let path = folder.join(opening::file_name(name));
        let mut file = new_file(&path).map_err(failed)?;
        file.write_all(bytes).and_then(|()| file.sync_all()).map_err(failed)?;
        drop(file);
        mailcraft_engine::mark::mark_from_internet(&path);
        // A copy: what is changed in it is not the message's (Save As keeps a file).
        if let Ok(meta) = fs::metadata(&path) {
            let mut perms = meta.permissions();
            perms.set_readonly(true);
            let _ = fs::set_permissions(&path, perms);
        }
        Ok(path)
    }

    /// This run's folder, made the first time a file is opened.
    fn run_folder(&self) -> std::io::Result<PathBuf> {
        let mut run = self.run.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(folder) = run.as_ref() {
            return Ok(folder.clone());
        }
        private_dirs(&self.base)?;
        let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
        let mut last = std::io::Error::other("no folder");
        for attempt in 0..16u32 {
            let folder = self.base.join(format!("{}-{stamp}-{attempt}", std::process::id()));
            match private_dir(&folder) {
                Ok(()) => {
                    *run = Some(folder.clone());
                    return Ok(folder);
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// Remove this run's folder (when MailCraft quits). A file another program still holds
    /// (Windows) stays, for the next start to tidy.
    pub fn clear(&self) {
        let run = self.run.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
        if let Some(folder) = run {
            remove_tree(&folder, DEPTH - 1);
        }
    }

    /// Remove what earlier runs left that is older than `max_age` (at a start). This run's own
    /// folder stays; a link among them is removed without following it.
    pub fn tidy(&self, max_age: Duration) {
        let mine = self.run.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let Ok(meta) = fs::symlink_metadata(&self.base) else { return };
        if !meta.is_dir() {
            return;
        }
        let Ok(entries) = fs::read_dir(&self.base) else { return };
        let now = SystemTime::now();
        for entry in entries.flatten().take(10_000) {
            let path = entry.path();
            if mine.as_deref() == Some(path.as_path()) {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&path) else { continue };
            let age = meta.modified().ok().and_then(|m| now.duration_since(m).ok()).unwrap_or_default();
            if age >= max_age {
                remove_tree(&path, DEPTH - 1);
            }
        }
    }
}

/// `path` and the folders above it that are missing, the last for this user only. A link at
/// `path` is refused.
fn private_dirs(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match private_dir(path) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let meta = fs::symlink_metadata(path)?;
            if !meta.is_dir() {
                return Err(std::io::Error::other("not a folder"));
            }
            restrict(path);
            Ok(())
        }
        other => other,
    }
}

/// A new folder for this user only; an error if anything is there already.
fn private_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(path)
}

/// Only this user may go into the folder (Unix; on Windows the user's own profile folder is).
fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// A file made new, for this user only; an error if anything is there already.
fn new_file(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

/// Remove `path`: a file or a link as it is (a link is never followed), a folder with what is in
/// it, `depth` levels at most.
fn remove_tree(path: &Path, depth: usize) {
    let Ok(meta) = fs::symlink_metadata(path) else { return };
    if !meta.is_dir() {
        // Windows won't remove a read-only file.
        if meta.permissions().readonly() {
            let mut perms = meta.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = fs::set_permissions(path, perms);
        }
        let _ = fs::remove_file(path);
        return;
    }
    if depth > 0
        && let Ok(entries) = fs::read_dir(path)
    {
        for entry in entries.flatten().take(10_000) {
            remove_tree(&entry.path(), depth - 1);
        }
    }
    let _ = fs::remove_dir(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mailcraft-opened-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("MailCraft").join("Attachments")
    }

    #[test]
    fn files_are_written_safely_and_cleared_on_exit() {
        let base = scratch("write");
        let opened = Opened::at(base.clone());
        let a = opened.write("../../evil/report.pdf", b"one").unwrap();
        let b = opened.write("report.pdf", b"two").unwrap();
        // Inside our own folder, under the leaf of the name, never on top of each other.
        assert!(a.starts_with(&base) && b.starts_with(&base) && a != b, "{a:?} {b:?}");
        assert_eq!(a.file_name().unwrap(), "report.pdf");
        assert_eq!((fs::read(&a).unwrap(), fs::read(&b).unwrap()), (b"one".to_vec(), b"two".to_vec()));
        assert!(fs::metadata(&a).unwrap().permissions().readonly());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(fs::metadata(&base).unwrap().permissions().mode() & 0o777, 0o700);
            assert_eq!(fs::metadata(a.parent().unwrap().parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        }
        // Programs are refused here too, whatever the window did.
        assert!(opened.write("setup.exe", b"MZ").is_err());
        assert!(opened.write("noextension", b"x").is_err());
        opened.clear();
        assert!(!a.exists() && !b.exists());
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        let _ = fs::remove_dir_all(base.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn old_runs_are_tidied_and_this_one_is_kept() {
        let base = scratch("tidy");
        let opened = Opened::at(base.clone());
        let mine = opened.write("notes.txt", b"x").unwrap();
        // Something an earlier run left.
        let old = base.join("1-2-0").join("1");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("left.txt"), b"y").unwrap();
        // Not old enough yet: kept.
        opened.tidy(MAX_AGE);
        assert!(old.join("left.txt").exists());
        // Old enough: gone, and this run's file stays.
        opened.tidy(Duration::ZERO);
        assert!(!base.join("1-2-0").exists() && mine.exists());
        opened.clear();
        let _ = fs::remove_dir_all(base.parent().unwrap().parent().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_never_followed() {
        let base = scratch("link");
        let outside = base.parent().unwrap().parent().unwrap().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep.txt"), b"keep").unwrap();
        fs::create_dir_all(base.parent().unwrap()).unwrap();
        // The folder for all runs is a link somewhere else: nothing is written.
        std::os::unix::fs::symlink(&outside, &base).unwrap();
        let opened = Opened::at(base.clone());
        assert!(opened.write("report.pdf", b"x").is_err());
        fs::remove_file(&base).unwrap();
        // A link among the runs is removed by tidying without what it points to going.
        fs::create_dir_all(&base).unwrap();
        std::os::unix::fs::symlink(&outside, base.join("9-9-9")).unwrap();
        opened.tidy(Duration::ZERO);
        assert!(!base.join("9-9-9").exists() && outside.join("keep.txt").exists());
        let _ = fs::remove_dir_all(base.parent().unwrap().parent().unwrap());
    }
}
